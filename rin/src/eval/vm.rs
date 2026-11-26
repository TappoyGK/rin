//! バイトコード命令の非再帰実行ループとスタックフレーム巻き戻し処理を実装する。

use crate::ast::Pattern;
use crate::bytecode::{BuiltinId, Op};
use crate::diagnostic::{write_available, write_suggest};
use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::mem::{RawVec, WorkBox};
use crate::pool::{Obj, ObjSlice, RVal};

use super::builtins::callbacks::{BuiltinFrame, CallbackStep};
use super::{CallOrigin, FrameSource, Runner};

mod calls;
mod execute;
mod pattern;
mod trace;

/// 作業領域の呼び出しフレームスタックに積む軽量 Rin フレーム（64 バイト級）。
#[derive(Clone, Copy)]
pub(super) struct RinFrame<'a> {
    pub(super) entry: FrameSource<'a>,
    pub(super) tail_source: Option<FrameSource<'a>>,
    pub(super) captures: *const RVal,
    pub(super) tail_calls: u32,
    pub(super) base: u32,
    pub(super) frame_stack_start: u32,
    pub(super) ip: u32,
    pub(super) cleanup_stack_to: u32,
    pub(super) origin_line: u32,
    pub(super) origin_col: u32,
    pub(super) caller_module: u32,
    pub(super) prev_active_module: u32,
    pub(super) origin_kind: u8,
    pub(super) origin_builtin: u8,
    pub(super) counted_depth: bool,
}

const NO_MODULE: u32 = u32::MAX;
const ORIGIN_NONE: u8 = 0;
const ORIGIN_RIN: u8 = 1;
const ORIGIN_HOST: u8 = 2;
const ORIGIN_BUILTIN: u8 = 3;

impl<'a> RinFrame<'a> {
    #[inline(always)]
    pub(super) fn new(
        entry: FrameSource<'a>,
        captures: ObjSlice<RVal>,
        base: usize,
        frame_stack_start: usize,
        origin: Option<CallOrigin>,
        caller_module: Option<u32>,
        prev_active_module: Option<u32>,
        cleanup_stack_to: usize,
        counted_depth: bool,
    ) -> Self {
        let (origin_kind, origin_line, origin_col, origin_builtin) = match origin {
            None => (ORIGIN_NONE, 0, 0, 0),
            Some(CallOrigin::Rin(s)) => (ORIGIN_RIN, s.line as u32, s.col as u32, 0),
            Some(CallOrigin::Host) => (ORIGIN_HOST, 0, 0, 0),
            Some(CallOrigin::Builtin { id, at }) => {
                (ORIGIN_BUILTIN, at.line as u32, at.col as u32, id as u8)
            }
        };
        Self {
            entry,
            tail_source: None,
            captures: captures.as_slice().as_ptr(),
            tail_calls: 0,
            base: base as u32,
            frame_stack_start: frame_stack_start as u32,
            ip: 0,
            cleanup_stack_to: cleanup_stack_to as u32,
            origin_line,
            origin_col,
            caller_module: caller_module.unwrap_or(NO_MODULE),
            prev_active_module: prev_active_module.unwrap_or(NO_MODULE),
            origin_kind,
            origin_builtin,
            counted_depth,
        }
    }

    #[inline(always)]
    pub(super) fn prev_mod(&self) -> Option<u32> {
        (self.prev_active_module != NO_MODULE).then_some(self.prev_active_module)
    }

    #[inline(always)]
    pub(super) fn caller_mod(&self) -> Option<u32> {
        (self.caller_module != NO_MODULE).then_some(self.caller_module)
    }

    #[inline(always)]
    pub(super) fn origin(&self) -> Option<CallOrigin> {
        match self.origin_kind {
            ORIGIN_RIN => Some(CallOrigin::Rin(Span {
                line: self.origin_line as usize,
                col: self.origin_col as usize,
            })),
            ORIGIN_HOST => Some(CallOrigin::Host),
            ORIGIN_BUILTIN => Some(CallOrigin::Builtin {
                id: BuiltinId::ALL[self.origin_builtin as usize],
                at: Span {
                    line: self.origin_line as usize,
                    col: self.origin_col as usize,
                },
            }),
            _ => None,
        }
    }

    #[inline(always)]
    pub(super) fn capture_at(&self, idx: usize) -> RVal {
        // SAFETY: コンパイラが `idx < proto.captures.len()` を保証しており、
        // `captures` はクロージャ生存中（スタック上または親フレーム保持中）有効。
        unsafe { *self.captures.add(idx) }
    }
}

pub(super) enum FrameEntry<'a> {
    Rin(RinFrame<'a>),
    Builtin(WorkBox<BuiltinFrame>),
}

pub(super) enum FrameAction {
    Done(RVal),
    ReturnToBuiltin(RVal),
    FailUnwound(ErrRecorded),
    CallCallbackBuiltin {
        builtin: BuiltinId,
        arg_start: usize,
        argc: usize,
        span: Span,
    },
}

enum DeliverAction<'f> {
    Done(RVal),
    ResumeTopRin,
    EnterRin(RinFrame<'f>),
}

impl<'a> Runner<'a, '_> {
    /// 最初のフレームを受け取り、ネイティブスタックを再帰させずに VM 全体を実行する。
    pub(super) fn run_vm<'f>(&mut self, initial: RinFrame<'f>) -> Result<RVal, ErrRecorded> {
        self.drive_frames(RawVec::new(), Some(initial))
    }

    /// ホストから直接高階組み込み関数を呼んだときの非再帰エントリポイント。
    pub(super) fn run_callback_builtin_entry(
        &mut self,
        b: BuiltinId,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let cleanup_to = self.stack.len();
        let (frame_opt, step) = self.start_callback_builtin(b, args, span, cleanup_to)?;
        let Some(bframe) = frame_opt else {
            let CallbackStep::Done(v) = step else {
                unreachable!();
            };
            return Ok(v);
        };
        let mut frames = RawVec::<FrameEntry<'_>>::new();
        let mut boxed = match WorkBox::try_new(&mut self.pool.work, &mut self.pool.core, bframe) {
            Ok(b) => b,
            Err(mut bf) => {
                let cleanup_to = bf.cleanup_stack_to;
                self.cleanup_builtin_frame(&mut bf);
                self.release_stack_from(cleanup_to);
                return Err(self.work_oom(span));
            }
        };
        if frames
            .reserve_work(&mut self.pool.work, &mut self.pool.core, 1)
            .is_none()
        {
            let cleanup_to = boxed.cleanup_stack_to;
            self.cleanup_builtin_frame(boxed.as_mut());
            self.release_stack_from(cleanup_to);
            return Err(self.work_oom(span));
        }
        let _ = frames.push_work(
            &mut self.pool.work,
            &mut self.pool.core,
            FrameEntry::Builtin(boxed),
        );
        let next = match self.handle_callback_step(&mut frames, step) {
            Ok(n) => n,
            Err(err) => return Err(self.unwind_frames(frames, err)),
        };
        match next {
            Some(child) => self.drive_frames(frames, Some(child)),
            None => {
                if frames.is_empty() {
                    return Ok(self.stack.pop().unwrap_or(RVal::Void));
                }
                self.drive_frames(frames, None)
            }
        }
    }

    fn drive_frames<'f>(
        &mut self,
        mut frames: RawVec<FrameEntry<'f>>,
        mut current: Option<RinFrame<'f>>,
    ) -> Result<RVal, ErrRecorded> {
        loop {
            let mut frame = if let Some(f) = current.take() {
                f
            } else {
                let Some(top) = frames.pop() else {
                    return Ok(RVal::Void);
                };
                match top {
                    FrameEntry::Rin(f) => f,
                    FrameEntry::Builtin(mut bframe) => {
                        self.cleanup_builtin_frame(bframe.as_mut());
                        unreachable!("BuiltinFrame cannot be top without a return value");
                    }
                }
            };

            let action = match self.exec_rin_frame(&mut frame, &mut frames) {
                Ok(a) => a,
                Err(err) => {
                    self.unwind_rin_frame(frame);
                    return Err(self.unwind_frames(frames, err));
                }
            };
            match action {
                FrameAction::Done(val) => return Ok(val),
                FrameAction::FailUnwound(err) => return Err(self.unwind_frames(frames, err)),
                FrameAction::ReturnToBuiltin(val) => match self.deliver_to_builtin(&mut frames, val)? {
                    DeliverAction::Done(done) => return Ok(done),
                    DeliverAction::ResumeTopRin => {}
                    DeliverAction::EnterRin(child) => current = Some(child),
                },
                FrameAction::CallCallbackBuiltin {
                    builtin,
                    arg_start,
                    argc,
                    span,
                } => {
                    if frames
                        .reserve_work(&mut self.pool.work, &mut self.pool.core, 1)
                        .is_none()
                    {
                        let err = self.work_oom(span);
                        self.unwind_rin_frame(frame);
                        return Err(self.unwind_frames(frames, err));
                    }
                    let _ = frames.push_work(
                        &mut self.pool.work,
                        &mut self.pool.core,
                        FrameEntry::Rin(frame),
                    );
                    match self.start_callback_from_stack(&mut frames, builtin, arg_start, argc, span) {
                        Ok(Some(child)) => current = Some(child),
                        Ok(None) => {}
                        Err(err) => return Err(self.unwind_frames(frames, err)),
                    }
                }
            }
        }
    }

    #[inline(always)]
    fn deliver_to_builtin<'f>(
        &mut self,
        frames: &mut RawVec<FrameEntry<'f>>,
        mut val: RVal,
    ) -> Result<DeliverAction<'f>, ErrRecorded> {
        loop {
            let Some(parent) = frames.last_mut() else {
                return Ok(DeliverAction::Done(val));
            };
            match parent {
                FrameEntry::Rin(_) => {
                    if let Err(err) = self.stack_push(val, Span::default()) {
                        let taken = std::mem::take(frames);
                        return Err(self.unwind_frames(taken, err));
                    }
                    return Ok(DeliverAction::ResumeTopRin);
                }
                FrameEntry::Builtin(bframe) => {
                    let step = match self.resume_callback_builtin(bframe.as_mut(), val) {
                        Ok(s) => s,
                        Err(err) => {
                            let taken = std::mem::take(frames);
                            return Err(self.unwind_frames(taken, err));
                        }
                    };
                    match step {
                        CallbackStep::Done(done_val) => {
                            let Some(FrameEntry::Builtin(mut done_frame)) = frames.pop() else {
                                unreachable!();
                            };
                            let cleanup_to = done_frame.cleanup_stack_to;
                            self.cleanup_builtin_frame(done_frame.as_mut());
                            self.release_stack_from(cleanup_to);
                            val = done_val;
                        }
                        CallbackStep::Invoke { .. } => {
                            match self.handle_callback_step(frames, step) {
                                Ok(Some(child)) => return Ok(DeliverAction::EnterRin(child)),
                                Ok(None) => return Ok(DeliverAction::ResumeTopRin),
                                Err(err) => {
                                    let taken = std::mem::take(frames);
                                    return Err(self.unwind_frames(taken, err));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn start_callback_from_stack<'f>(
        &mut self,
        frames: &mut RawVec<FrameEntry<'f>>,
        builtin: BuiltinId,
        arg_start: usize,
        argc: usize,
        span: Span,
    ) -> Result<Option<RinFrame<'f>>, ErrRecorded> {
        if !self.try_charge(1) {
            self.release_stack_from(arg_start - 1);
            return self.step_limit_result(span);
        }
        // 引数は最大 3 個（fold）なので、3 個以下はスタック配列でコピーして WorkArena 確保を避ける。
        let mut stack_args = [RVal::Void; 4];
        let mut overflow_args = RawVec::<RVal>::new();
        let args_slice: &[RVal] = if argc <= stack_args.len() {
            let src = &self.stack.as_slice()[arg_start..arg_start + argc];
            stack_args[..argc].copy_from_slice(src);
            self.stack.truncate(arg_start - 1);
            &stack_args[..argc]
        } else {
            if overflow_args
                .reserve_work(&mut self.pool.work, &mut self.pool.core, argc)
                .is_none()
            {
                self.release_stack_from(arg_start - 1);
                return Err(self.work_oom(span));
            }
            let src = &self.stack.as_slice()[arg_start..arg_start + argc];
            let _ =
                overflow_args.extend_from_slice_work(&mut self.pool.work, &mut self.pool.core, src);
            self.stack.truncate(arg_start - 1);
            overflow_args.as_slice()
        };

        let res = self.start_callback_builtin(builtin, args_slice, span, arg_start - 1);
        self.release_slice(args_slice);
        let (frame_opt, step) = res?;
        match frame_opt {
            None => {
                let CallbackStep::Done(val) = step else {
                    unreachable!();
                };
                self.stack_push(val, span)?;
                Ok(None)
            }
            Some(bframe) => {
                let mut boxed =
                    match WorkBox::try_new(&mut self.pool.work, &mut self.pool.core, bframe) {
                        Ok(b) => b,
                        Err(mut bf) => {
                            let cleanup_to = bf.cleanup_stack_to;
                            self.cleanup_builtin_frame(&mut bf);
                            self.release_stack_from(cleanup_to);
                            return Err(self.work_oom(span));
                        }
                    };
                if frames
                    .reserve_work(&mut self.pool.work, &mut self.pool.core, 1)
                    .is_none()
                {
                    let cleanup_to = boxed.cleanup_stack_to;
                    self.cleanup_builtin_frame(boxed.as_mut());
                    self.release_stack_from(cleanup_to);
                    return Err(self.work_oom(span));
                }
                let _ = frames.push_work(
                    &mut self.pool.work,
                    &mut self.pool.core,
                    FrameEntry::Builtin(boxed),
                );
                self.handle_callback_step(frames, step)
            }
        }
    }

    /// `CallbackStep::Invoke` を処理する。
    /// Rin クロージャなら `Some(child)` を返して即座に実行し、同期組み込みならループで進める。
    #[inline(always)]
    fn handle_callback_step<'f>(
        &mut self,
        frames: &mut RawVec<FrameEntry<'f>>,
        mut step: CallbackStep,
    ) -> Result<Option<RinFrame<'f>>, ErrRecorded> {
        loop {
            let CallbackStep::Invoke {
                callee,
                args,
                argc,
                span,
                builtin_id,
            } = step
            else {
                return Ok(None);
            };
            let arg_slice = &args[..argc];
            match callee {
                RVal::Obj(id) => {
                    let (proto, captures) = match self.pool.get(id) {
                        Obj::Closure { proto, captures } => (*proto, *captures),
                        _ => return Err(self.call_type_error(callee, span)),
                    };
                    if proto.arity as usize != argc {
                        return Err(self.call_arity_error(proto.arity, argc, span));
                    }
                    self.enter_depth(span)?;
                    let prev_mod = std::mem::replace(&mut self.active_module, proto.module);
                    let base = self.stack.len();
                    let local_count = proto.local_count as usize;
                    if let Err(e) = self.stack_reserve(local_count, span) {
                        self.active_module = prev_mod;
                        self.leave_depth();
                        return Err(e);
                    }
                    for &v in arg_slice {
                        self.pool.retain_val(v);
                        let _ = self
                            .stack
                            .push_work(&mut self.pool.work, &mut self.pool.core, v);
                    }
                    let _ = self.stack.resize_work(
                        &mut self.pool.work,
                        &mut self.pool.core,
                        base + local_count,
                        RVal::Void,
                    );
                    let child = RinFrame::new(
                        FrameSource::Proto(proto),
                        captures,
                        base,
                        base + local_count,
                        Some(CallOrigin::Builtin {
                            id: builtin_id,
                            at: span,
                        }),
                        prev_mod,
                        prev_mod,
                        base,
                        true,
                    );
                    return Ok(Some(child));
                }
                RVal::Builtin(b) => {
                    if !self.try_charge(1) {
                        return self.step_limit_result(span);
                    }
                    if b.has_callback() {
                        let cleanup_to = self.stack.len();
                        let (sub_frame, sub_step) =
                            self.start_callback_builtin(b, arg_slice, span, cleanup_to)?;
                        match sub_frame {
                            Some(sf) => {
                                let mut boxed = match WorkBox::try_new(
                                    &mut self.pool.work,
                                    &mut self.pool.core,
                                    sf,
                                ) {
                                    Ok(b) => b,
                                    Err(mut bf) => {
                                        let cleanup_to = bf.cleanup_stack_to;
                                        self.cleanup_builtin_frame(&mut bf);
                                        self.release_stack_from(cleanup_to);
                                        return Err(self.work_oom(span));
                                    }
                                };
                                if frames
                                    .reserve_work(&mut self.pool.work, &mut self.pool.core, 1)
                                    .is_none()
                                {
                                    let cleanup_to = boxed.cleanup_stack_to;
                                    self.cleanup_builtin_frame(boxed.as_mut());
                                    self.release_stack_from(cleanup_to);
                                    return Err(self.work_oom(span));
                                }
                                let _ = frames.push_work(
                                    &mut self.pool.work,
                                    &mut self.pool.core,
                                    FrameEntry::Builtin(boxed),
                                );
                                step = sub_step;
                                continue;
                            }
                            None => {
                                let CallbackStep::Done(val) = sub_step else {
                                    unreachable!();
                                };
                                match self.advance_builtin_with_value(frames, val)? {
                                    Some(next_step) => {
                                        step = next_step;
                                        continue;
                                    }
                                    None => return Ok(None),
                                }
                            }
                        }
                    } else {
                        let val = self.apply_non_callback_builtin(b, arg_slice, span)?;
                        match self.advance_builtin_with_value(frames, val)? {
                            Some(next_step) => {
                                step = next_step;
                                continue;
                            }
                            None => return Ok(None),
                        }
                    }
                }
                other => return Err(self.call_type_error(other, span)),
            }
        }
    }

    /// 同期組み込み呼び出しの結果を直近の `BuiltinFrame` へ渡し、完了したら親へ伝播する。
    fn advance_builtin_with_value<'f>(
        &mut self,
        frames: &mut RawVec<FrameEntry<'f>>,
        mut val: RVal,
    ) -> Result<Option<CallbackStep>, ErrRecorded> {
        loop {
            let Some(parent) = frames.last_mut() else {
                self.stack_push(val, Span::default())?;
                return Ok(None);
            };
            match parent {
                FrameEntry::Rin(_) => {
                    self.stack_push(val, Span::default())?;
                    return Ok(None);
                }
                FrameEntry::Builtin(bframe) => {
                    let next = self.resume_callback_builtin(bframe.as_mut(), val)?;
                    match next {
                        CallbackStep::Done(done_val) => {
                            let Some(FrameEntry::Builtin(mut done_frame)) = frames.pop() else {
                                unreachable!();
                            };
                            let cleanup_to = done_frame.cleanup_stack_to;
                            self.cleanup_builtin_frame(done_frame.as_mut());
                            self.release_stack_from(cleanup_to);
                            val = done_val;
                        }
                        CallbackStep::Invoke { .. } => return Ok(Some(next)),
                    }
                }
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn unwind_rin_frame(&mut self, frame: RinFrame<'_>) {
        self.record_unwind_frame(
            &frame.entry,
            frame.tail_source.as_ref(),
            frame.tail_calls as usize,
            frame.origin(),
            frame.caller_mod(),
        );
        self.active_module = frame.prev_mod();
        if frame.counted_depth {
            self.leave_depth();
        }
        self.release_stack_from(frame.cleanup_stack_to as usize);
    }

    #[cold]
    #[inline(never)]
    fn unwind_frames<'f>(
        &mut self,
        mut frames: RawVec<FrameEntry<'f>>,
        err: ErrRecorded,
    ) -> ErrRecorded {
        while let Some(entry) = frames.pop() {
            match entry {
                FrameEntry::Rin(frame) => {
                    self.unwind_rin_frame(frame);
                }
                FrameEntry::Builtin(mut bframe) => {
                    let cleanup_to = bframe.cleanup_stack_to;
                    self.cleanup_builtin_frame(bframe.as_mut());
                    self.release_stack_from(cleanup_to);
                }
            }
        }
        err
    }
}
