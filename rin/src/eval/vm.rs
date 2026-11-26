//! バイトコード命令の非再帰実行ループとスタックフレーム巻き戻し処理を実装する。

use crate::ast::Pattern;
use crate::bytecode::{BuiltinId, Op};
use crate::diagnostic::{write_available, write_suggest};
use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::mem::RawVec;
use crate::pool::{Obj, ObjSlice, RVal};

use super::builtins::callbacks::{BuiltinFrame, CallbackStep};
use super::{CallOrigin, FrameSource, Runner};

mod calls;
mod execute;
mod pattern;
mod trace;

/// 作業領域の呼び出しフレームスタックに積む Rin フレーム。
pub(super) struct RinFrame<'a> {
    pub(super) entry: FrameSource<'a>,
    pub(super) tail_source: Option<FrameSource<'a>>,
    pub(super) tail_calls: usize,
    pub(super) captures: ObjSlice<RVal>,
    pub(super) base: usize,
    pub(super) frame_stack_start: usize,
    pub(super) ip: usize,
    pub(super) origin: Option<CallOrigin>,
    pub(super) caller_module: Option<u32>,
    pub(super) prev_active_module: Option<u32>,
    pub(super) cleanup_stack_to: usize,
    pub(super) counted_depth: bool,
}

pub(super) enum FrameEntry<'a> {
    Rin(RinFrame<'a>),
    Builtin(BuiltinFrame),
}

pub(super) enum FrameAction {
    Return(RVal),
    CallClosure {
        arg_start: usize,
        argc: usize,
        span: Span,
    },
    CallCallbackBuiltin {
        builtin: BuiltinId,
        arg_start: usize,
        argc: usize,
        span: Span,
    },
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
        if frames
            .reserve_work(&mut self.pool.work, &mut self.pool.core, 1)
            .is_none()
        {
            let cleanup_to = bframe.cleanup_stack_to;
            self.cleanup_builtin_frame(bframe);
            self.release_stack_from(cleanup_to);
            return Err(self.work_oom(span));
        }
        let _ = frames.push_work(
            &mut self.pool.work,
            &mut self.pool.core,
            FrameEntry::Builtin(bframe),
        );
        if let Err(err) = self.handle_callback_step(&mut frames, step) {
            return Err(self.unwind_frames(frames, err));
        }
        if frames.is_empty() {
            return Ok(self.stack.pop().unwrap_or(RVal::Void));
        }
        self.drive_frames(frames, None)
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
                    FrameEntry::Builtin(bframe) => {
                        self.cleanup_builtin_frame(bframe);
                        unreachable!("BuiltinFrame cannot be top without a return value");
                    }
                }
            };

            let action = match self.exec_rin_frame(&mut frame) {
                Ok(a) => a,
                Err(err) => {
                    self.unwind_rin_frame(frame);
                    return Err(self.unwind_frames(frames, err));
                }
            };
            match action {
                FrameAction::Return(val) => {
                    self.active_module = frame.prev_active_module;
                    if frame.counted_depth {
                        self.leave_depth();
                    }
                    self.release_stack_from(frame.cleanup_stack_to);
                    if let Some(done) = self.deliver_return(&mut frames, val)? {
                        return Ok(done);
                    }
                }
                FrameAction::CallClosure {
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
                    if let Err(err) =
                        self.push_closure_call_from_stack(&mut frames, arg_start, argc, span)
                    {
                        return Err(self.unwind_frames(frames, err));
                    }
                }
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
                    if let Err(err) = self.start_callback_from_stack(
                        &mut frames,
                        builtin,
                        arg_start,
                        argc,
                        span,
                    ) {
                        return Err(self.unwind_frames(frames, err));
                    }
                }
            }
        }
    }

    fn deliver_return<'f>(
        &mut self,
        frames: &mut RawVec<FrameEntry<'f>>,
        mut val: RVal,
    ) -> Result<Option<RVal>, ErrRecorded> {
        loop {
            let Some(parent) = frames.last_mut() else {
                return Ok(Some(val));
            };
            match parent {
                FrameEntry::Rin(_) => {
                    if let Err(err) = self.stack_push(val, Span::default()) {
                        let taken = std::mem::take(frames);
                        return Err(self.unwind_frames(taken, err));
                    }
                    return Ok(None);
                }
                FrameEntry::Builtin(bframe) => {
                    let step = match self.resume_callback_builtin(bframe, val) {
                        Ok(s) => s,
                        Err(err) => {
                            let taken = std::mem::take(frames);
                            return Err(self.unwind_frames(taken, err));
                        }
                    };
                    match step {
                        CallbackStep::Done(done_val) => {
                            let Some(FrameEntry::Builtin(done_frame)) = frames.pop() else {
                                unreachable!();
                            };
                            let cleanup_to = done_frame.cleanup_stack_to;
                            self.cleanup_builtin_frame(done_frame);
                            self.release_stack_from(cleanup_to);
                            val = done_val;
                            // さらに親フレームへ返す。
                        }
                        CallbackStep::Invoke { .. } => {
                            if let Err(err) = self.handle_callback_step(frames, step) {
                                let taken = std::mem::take(frames);
                                return Err(self.unwind_frames(taken, err));
                            }
                            // もし同期組み込み呼び出しで `Done` まで進んだ場合は `handle_callback_step` 内で処理されない
                            // （`handle_callback_step` が次の `RinFrame` を積むか、ここで同期処理する）。
                            return Ok(None);
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
    ) -> Result<(), ErrRecorded> {
        if !self.try_charge(1) {
            self.release_stack_from(arg_start - 1);
            return self.step_limit_result(span);
        }
        // スタック上の引数をコピーして取り出し、スタックを `arg_start - 1` まで戻す。
        let mark = self.pool.work.mark(&self.pool.core);
        let mut args_buf = RawVec::<RVal>::new();
        if args_buf
            .reserve_work(&mut self.pool.work, &mut self.pool.core, argc)
            .is_none()
        {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.release_stack_from(arg_start - 1);
            return Err(self.work_oom(span));
        }
        let src = &self.stack.as_slice()[arg_start..arg_start + argc];
        let _ = args_buf.extend_from_slice_work(&mut self.pool.work, &mut self.pool.core, src);
        self.stack.truncate(arg_start - 1);

        let res = self.start_callback_builtin(builtin, args_buf.as_slice(), span, arg_start - 1);
        // `start_callback_builtin` は必要な引数（`f`, `list_v`, `init`）を独自に retain 済み。
        self.release_slice(args_buf.as_slice());
        // 注意: `start_callback_builtin` が `items` を作業領域へ確保しているため、ここでは `mark` へ戻さない。
        let (frame_opt, step) = res?;
        match frame_opt {
            None => {
                let CallbackStep::Done(val) = step else {
                    unreachable!();
                };
                self.stack_push(val, span)?;
                Ok(())
            }
            Some(bframe) => {
                if frames
                    .reserve_work(&mut self.pool.work, &mut self.pool.core, 1)
                    .is_none()
                {
                    let cleanup_to = bframe.cleanup_stack_to;
                    self.cleanup_builtin_frame(bframe);
                    self.release_stack_from(cleanup_to);
                    return Err(self.work_oom(span));
                }
                let _ = frames.push_work(
                    &mut self.pool.work,
                    &mut self.pool.core,
                    FrameEntry::Builtin(bframe),
                );
                self.handle_callback_step(frames, step)
            }
        }
    }

    /// `CallbackStep::Invoke` を処理する。同期組み込み関数ならループで即座に処理し、
    /// Rin クロージャなら新しい `RinFrame` を積んで VM ループへ戻す。
    fn handle_callback_step<'f>(
        &mut self,
        frames: &mut RawVec<FrameEntry<'f>>,
        mut step: CallbackStep,
    ) -> Result<(), ErrRecorded> {
        loop {
            let CallbackStep::Invoke {
                callee,
                args,
                argc,
                span,
                builtin_name,
            } = step
            else {
                return Ok(());
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
                    let child = RinFrame {
                        entry: FrameSource::Proto(proto),
                        tail_source: None,
                        tail_calls: 0,
                        captures,
                        base,
                        frame_stack_start: base + local_count,
                        ip: 0,
                        origin: Some(CallOrigin::Builtin {
                            name: builtin_name,
                            at: span,
                        }),
                        caller_module: prev_mod,
                        prev_active_module: prev_mod,
                        cleanup_stack_to: base,
                        counted_depth: true,
                    };
                    if frames
                        .push_work(
                            &mut self.pool.work,
                            &mut self.pool.core,
                            FrameEntry::Rin(child),
                        )
                        .is_none()
                    {
                        self.active_module = prev_mod;
                        self.leave_depth();
                        self.release_stack_from(base);
                        return Err(self.work_oom(span));
                    }
                    return Ok(());
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
                                if frames
                                    .reserve_work(&mut self.pool.work, &mut self.pool.core, 1)
                                    .is_none()
                                {
                                    let cleanup_to = sf.cleanup_stack_to;
                                    self.cleanup_builtin_frame(sf);
                                    self.release_stack_from(cleanup_to);
                                    return Err(self.work_oom(span));
                                }
                                let _ = frames.push_work(
                                    &mut self.pool.work,
                                    &mut self.pool.core,
                                    FrameEntry::Builtin(sf),
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
                                    None => return Ok(()),
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
                            None => return Ok(()),
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
                // 最上位が `BuiltinFrame` で完了した場合：結果をスタックへ積まず呼び出し元へ返すため
                // ここではダミーの `Done` は不要だが、`drive_frames` 経由なので `stack_push` するか
                // `run_callback_builtin_entry` の場合は空になる。
                self.stack_push(val, Span::default())?;
                return Ok(None);
            };
            match parent {
                FrameEntry::Rin(_) => {
                    self.stack_push(val, Span::default())?;
                    return Ok(None);
                }
                FrameEntry::Builtin(bframe) => {
                    let next = self.resume_callback_builtin(bframe, val)?;
                    match next {
                        CallbackStep::Done(done_val) => {
                            let Some(FrameEntry::Builtin(done_frame)) = frames.pop() else {
                                unreachable!();
                            };
                            let cleanup_to = done_frame.cleanup_stack_to;
                            self.cleanup_builtin_frame(done_frame);
                            self.release_stack_from(cleanup_to);
                            val = done_val;
                        }
                        CallbackStep::Invoke { .. } => return Ok(Some(next)),
                    }
                }
            }
        }
    }

    fn push_closure_call_from_stack<'f>(
        &mut self,
        frames: &mut RawVec<FrameEntry<'f>>,
        arg_start: usize,
        argc: usize,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        let callee = self.stack.as_slice()[arg_start - 1];
        let RVal::Obj(id) = callee else {
            return Err(self.call_type_error(callee, span));
        };
        let (proto, captures) = match self.pool.get(id) {
            Obj::Closure { proto, captures } => (*proto, *captures),
            _ => return Err(self.call_type_error(callee, span)),
        };
        if proto.arity as usize != argc {
            return Err(self.call_arity_error(proto.arity, argc, span));
        }
        self.enter_depth(span)?;
        let child_base = arg_start;
        let child_locals = proto.local_count as usize;
        if let Err(e) = self.stack_resize_void(child_base + child_locals, span) {
            self.leave_depth();
            return Err(e);
        }
        let prev_mod = std::mem::replace(&mut self.active_module, proto.module);
        let child = RinFrame {
            entry: FrameSource::Proto(proto),
            tail_source: None,
            tail_calls: 0,
            captures,
            base: child_base,
            frame_stack_start: child_base + child_locals,
            ip: 0,
            origin: Some(CallOrigin::Rin(span)),
            caller_module: prev_mod,
            prev_active_module: prev_mod,
            cleanup_stack_to: arg_start - 1,
            counted_depth: true,
        };
        if frames
            .push_work(
                &mut self.pool.work,
                &mut self.pool.core,
                FrameEntry::Rin(child),
            )
            .is_none()
        {
            self.active_module = prev_mod;
            self.leave_depth();
            self.release_stack_from(arg_start - 1);
            return Err(self.work_oom(span));
        }
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn unwind_rin_frame(&mut self, frame: RinFrame<'_>) {
        self.record_unwind_frame(
            &frame.entry,
            frame.tail_source.as_ref(),
            frame.tail_calls,
            frame.origin,
            frame.caller_module,
        );
        self.active_module = frame.prev_active_module;
        if frame.counted_depth {
            self.leave_depth();
        }
        self.release_stack_from(frame.cleanup_stack_to);
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
                FrameEntry::Builtin(bframe) => {
                    let cleanup_to = bframe.cleanup_stack_to;
                    self.cleanup_builtin_frame(bframe);
                    self.release_stack_from(cleanup_to);
                }
            }
        }
        err
    }
}
