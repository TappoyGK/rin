//! 評価ごとの状態操作、資源制限、束縛解決と呼び出し。
use std::fmt::Write;

use super::*;
use crate::mem::RawVec;
use crate::pool::ObjSlice;

impl<'a, 'p> Runner<'a, 'p> {
    #[cold]
    #[inline(never)]
    pub(super) fn err_runtime(&mut self, rule: &'static str, span: Span, msg: &str) -> ErrRecorded {
        self.pool.core.err_str(rule, ErrorKind::Runtime, span, msg)
    }

    #[cold]
    #[inline(never)]
    pub(super) fn err_runtime_fmt(
        &mut self,
        rule: &'static str,
        span: Span,
        args: std::fmt::Arguments<'_>,
    ) -> ErrRecorded {
        self.pool.core.err_fmt(rule, ErrorKind::Runtime, span, args)
    }

    #[inline(always)]
    pub(super) fn tick(
        &mut self,
        chunk: &Chunk,
        start: u32,
        count: u32,
    ) -> Result<(), ErrRecorded> {
        let remaining = self.steps_left;
        if count as usize > remaining {
            let span = chunk.tick_spans[start as usize + remaining];
            return self.step_limit_result(span);
        }
        self.steps_left -= count as usize;
        Ok(())
    }

    #[inline(always)]
    pub(super) fn try_charge(&mut self, amount: usize) -> bool {
        try_charge_steps(&mut self.steps_left, amount)
    }

    #[cold]
    #[inline(never)]
    pub(super) fn step_limit_err(&mut self, span: Span) -> ErrRecorded {
        let limit = self.limits.steps;
        self.pool.core.err_fmt(
            "eval.limits",
            ErrorKind::TooManySteps,
            span,
            format_args!("step limit exceeded ({limit})"),
        )
    }

    #[cold]
    #[inline(never)]
    pub(super) fn step_limit_result<T>(&mut self, span: Span) -> Result<T, ErrRecorded> {
        Err(self.step_limit_err(span))
    }

    #[cold]
    #[inline(never)]
    pub(super) fn work_oom(&mut self, span: Span) -> ErrRecorded {
        let max = self.pool.core.max_bytes();
        self.pool.core.err_fmt(
            "eval.limits",
            ErrorKind::TooMuchMemory,
            span,
            format_args!("memory limit exceeded ({max} bytes)"),
        )
    }

    #[inline(always)]
    pub(super) fn stack_push_at(
        &mut self,
        val: RVal,
        chunk: &Chunk,
        op_ip: usize,
    ) -> Result<(), ErrRecorded> {
        if self
            .stack
            .push_work(&mut self.pool.work, &mut self.pool.core, val)
            .is_none()
        {
            return Err(self.stack_push_oom(val, chunk, op_ip));
        }
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn stack_push_oom(&mut self, val: RVal, chunk: &Chunk, op_ip: usize) -> ErrRecorded {
        self.pool.release_val(val);
        self.work_oom(chunk.spans[op_ip])
    }

    #[inline(always)]
    pub(super) fn stack_push(&mut self, val: RVal, span: Span) -> Result<(), ErrRecorded> {
        if self
            .stack
            .push_work(&mut self.pool.work, &mut self.pool.core, val)
            .is_none()
        {
            self.pool.release_val(val);
            return Err(self.work_oom(span));
        }
        Ok(())
    }

    #[inline(always)]
    pub(super) fn stack_resize_void(
        &mut self,
        new_len: usize,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        if self
            .stack
            .resize_work(
                &mut self.pool.work,
                &mut self.pool.core,
                new_len,
                RVal::Void,
            )
            .is_none()
        {
            return Err(self.work_oom(span));
        }
        Ok(())
    }

    #[inline(always)]
    pub(super) fn stack_reserve(
        &mut self,
        additional: usize,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        if self
            .stack
            .reserve_work(&mut self.pool.work, &mut self.pool.core, additional)
            .is_none()
        {
            return Err(self.work_oom(span));
        }
        Ok(())
    }

    #[inline(always)]
    pub(super) fn enter_depth(&mut self, span: Span) -> Result<(), ErrRecorded> {
        self.depth += 1;
        if self.depth > self.limits.depth {
            return Err(self.depth_limit_err(span));
        }
        Ok(())
    }

    #[cold]
    #[inline(never)]
    pub(super) fn depth_limit_err(&mut self, span: Span) -> ErrRecorded {
        let limit = self.limits.depth;
        self.pool.core.err_fmt(
            "eval.limits",
            ErrorKind::TooDeep,
            span,
            format_args!("call depth limit exceeded ({limit})"),
        )
    }

    #[inline(always)]
    pub(super) fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    #[inline(always)]
    pub(super) fn lookup_global(
        &mut self,
        id: u32,
        chunk: &Chunk,
        ip: usize,
    ) -> Result<RVal, ErrRecorded> {
        if let Some(mod_id) = self.active_module
            && let Some(v) = self.modules.env(mod_id).get(id)
        {
            self.pool.retain_val(v);
            return Ok(v);
        }
        if let Some(v) = global_slot(self.globals, id) {
            self.pool.retain_val(v);
            return Ok(v);
        }
        if self.frozen
            && self.active_module.is_none()
            && let Some(name) = self.syms.name(id)
            && let Some(v) = self.modules.get_val(name)
        {
            self.pool.retain_val(v);
            return Ok(v);
        }
        Err(self.unknown_name(id, chunk, ip))
    }

    #[cold]
    #[inline(never)]
    fn unknown_name(&mut self, id: u32, chunk: &Chunk, ip: usize) -> ErrRecorded {
        let span = chunk.spans[ip];
        let name = self.syms.name(id).unwrap_or("?");
        let frozen_mod_names = (self.frozen && self.active_module.is_none())
            .then(|| self.modules.loaded_names())
            .into_iter()
            .flatten();
        let active_mod = self.active_module;
        let out = self
            .pool
            .core
            .error
            .begin_msg("expr.name", ErrorKind::Runtime, span);
        let _ = write!(out, "unknown name '{name}'");
        write_suggest(
            out,
            name,
            chunk
                .visible_names(ip)
                .chain(global_bound_names(self.syms, self.globals))
                .chain(frozen_mod_names)
                .chain(
                    active_mod
                        .into_iter()
                        .flat_map(|mid| env_bound_names(self.syms, self.modules.env(mid))),
                ),
        );
        ErrRecorded
    }

    pub(super) fn define_top(
        &mut self,
        id: u32,
        val: RVal,
        span: Span,
        rule: &'static str,
    ) -> Result<(), ErrRecorded> {
        let name = self.syms.name(id).unwrap_or("?");
        if id < STATIC_SYM_COUNT {
            self.pool.release_val(val);
            return Err(self.err_runtime_fmt(
                rule,
                span,
                format_args!("cannot redefine builtin '{name}'"),
            ));
        }
        if let Some(mod_id) = self.active_module {
            if self.modules.env(mod_id).get(id).is_some() {
                self.pool.release_val(val);
                return Err(self.err_runtime_fmt(
                    rule,
                    span,
                    format_args!("'{name}' is already defined"),
                ));
            }
            match self.modules.set_env_slot(self.pool, mod_id, id, val, span) {
                Ok(_) => Ok(()),
                Err(e) => {
                    self.pool.release_val(val);
                    Err(e)
                }
            }
        } else {
            if self.globals.get(id).is_some() {
                self.pool.release_val(val);
                return Err(self.err_runtime_fmt(
                    rule,
                    span,
                    format_args!("'{name}' is already defined"),
                ));
            }
            if self.modules.get_val(name).is_some() {
                self.pool.release_val(val);
                return Err(self.err_runtime_fmt(
                    rule,
                    span,
                    format_args!("'{name}' conflicts with loaded module '{name}'"),
                ));
            }
            if self
                .added_globals
                .push_work(&mut self.pool.work, &mut self.pool.core, id)
                .is_none()
            {
                self.pool.release_val(val);
                return Err(self.work_oom(span));
            }
            match self.globals.set(self.pool, id, val, span) {
                Ok(_) => Ok(()),
                Err(e) => {
                    self.added_globals.pop();
                    self.pool.release_val(val);
                    Err(e)
                }
            }
        }
    }

    pub(super) fn rollback_added_globals(&mut self) {
        for &id in self.added_globals.as_slice() {
            self.globals.unset_no_alloc(self.pool, id);
        }
        self.added_globals.clear();
    }

    pub(super) fn release_slice(&mut self, vals: &[RVal]) {
        for &v in vals {
            self.pool.release_val(v);
        }
    }

    #[inline(always)]
    pub(super) fn release_stack_from(&mut self, from: usize) {
        let len = self.stack.len();
        if len > from {
            let mut i = len;
            while i > from {
                i -= 1;
                let v = self.stack.get_copy(i);
                self.pool.release_val(v);
            }
            self.stack.truncate(from);
        }
    }

    pub(super) fn build_closure(
        &mut self,
        proto: FnProtoPtr,
        captures: *const RVal,
        base: usize,
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        if proto.captures.is_empty() {
            let id = self.pool.alloc_closure_owned(proto, &[], span)?;
            return Ok(RVal::Obj(id));
        }
        let mark = self.pool.work.mark(&self.pool.core);
        let mut cap_vals = RawVec::<RVal>::new();
        if cap_vals
            .reserve_work(
                &mut self.pool.work,
                &mut self.pool.core,
                proto.captures.len(),
            )
            .is_none()
        {
            self.pool.work.rewind(&mut self.pool.core, mark);
            return Err(self.work_oom(span));
        }
        for cap in proto.captures.as_slice() {
            let v = match cap {
                Capture::Local(slot) => self.stack.as_slice()[base + *slot as usize],
                // SAFETY: コンパイラが `idx < 親クロージャの captures.len()` を保証する。
                Capture::Upvalue(idx) => unsafe { *captures.add(*idx as usize) },
            };
            self.pool.retain_val(v);
            let _ = cap_vals.push_work(&mut self.pool.work, &mut self.pool.core, v);
        }
        let res = self
            .pool
            .alloc_closure_owned(proto, cap_vals.as_slice(), span);
        self.pool.work.rewind(&mut self.pool.core, mark);
        Ok(RVal::Obj(res?))
    }

    pub(super) fn run_top_chunk(&mut self, chunk: &Chunk) -> Result<RVal, ErrRecorded> {
        self.run_top_chunk_with_locals(chunk, &[])
    }

    pub(super) fn run_top_chunk_with_locals(
        &mut self,
        chunk: &Chunk,
        initial_vals: &[RVal],
    ) -> Result<RVal, ErrRecorded> {
        let base = self.stack.len();
        let local_count = chunk.local_count as usize;
        self.stack_reserve(local_count, Span::default())?;
        for &val in initial_vals {
            self.pool.retain_val(val);
            let _ = self
                .stack
                .push_work(&mut self.pool.work, &mut self.pool.core, val);
        }
        let _ = self.stack.resize_work(
            &mut self.pool.work,
            &mut self.pool.core,
            base + local_count,
            RVal::Void,
        );
        let frame = super::vm::RinFrame::new(
            FrameSource::Top(chunk),
            ObjSlice::empty(),
            base,
            base + local_count,
            None,
            self.active_module,
            self.active_module,
            base,
            false,
        );
        let res = self.run_vm(frame);
        if res.is_err() && !self.frozen && self.active_module.is_none() {
            self.rollback_added_globals();
        }
        res
    }

    #[cold]
    #[inline(never)]
    pub(super) fn call_type_error(&mut self, callee: RVal, span: Span) -> ErrRecorded {
        let tname = callee.type_name(self.pool);
        self.err_runtime_fmt(
            "expr.call",
            span,
            format_args!("cannot call value of type {tname}"),
        )
    }

    #[cold]
    #[inline(never)]
    pub(super) fn call_arity_error(
        &mut self,
        expected: u32,
        actual: usize,
        span: Span,
    ) -> ErrRecorded {
        self.err_runtime_fmt(
            "expr.call",
            span,
            format_args!("expected {expected} arguments, got {actual}"),
        )
    }

    pub(super) fn apply(
        &mut self,
        callee: RVal,
        args: &[RVal],
        span: Span,
        origin: CallOrigin,
    ) -> Result<RVal, ErrRecorded> {
        match callee {
            RVal::Obj(id) => {
                let (proto, captures) = match self.pool.get(id) {
                    Obj::Closure { proto, captures } => (*proto, *captures),
                    _ => return Err(self.call_type_error(callee, span)),
                };
                if proto.arity as usize != args.len() {
                    return Err(self.call_arity_error(proto.arity, args.len(), span));
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
                for &val in args {
                    self.pool.retain_val(val);
                    let _ = self
                        .stack
                        .push_work(&mut self.pool.work, &mut self.pool.core, val);
                }
                let _ = self.stack.resize_work(
                    &mut self.pool.work,
                    &mut self.pool.core,
                    base + local_count,
                    RVal::Void,
                );
                let frame = super::vm::RinFrame::new(
                    FrameSource::Proto(proto),
                    captures,
                    base,
                    base + local_count,
                    Some(origin),
                    prev_mod,
                    prev_mod,
                    base,
                    true,
                );
                self.run_vm(frame)
            }
            RVal::Builtin(b) => {
                if !self.try_charge(1) {
                    return self.step_limit_result(span);
                }
                if b.has_callback() {
                    self.run_callback_builtin_entry(b, args, span)
                } else {
                    self.apply_non_callback_builtin(b, args, span)
                }
            }
            other => Err(self.call_type_error(other, span)),
        }
    }
}

// 残量を先に検査してから減算する。usize::MAX でも桁あふれしない。
#[inline(always)]
pub(super) fn try_charge_steps(remaining: &mut usize, amount: usize) -> bool {
    if amount > *remaining {
        return false;
    }
    *remaining -= amount;
    true
}
