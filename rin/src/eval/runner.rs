//! 評価ごとの状態操作、資源制限、束縛解決と呼び出し。
use super::*;
use crate::mem::RawVec;
use crate::pool::ObjSlice;

impl<'a, 'p> Runner<'a, 'p> {
    #[inline(always)]
    pub(super) fn tick(&mut self, chunk: &Chunk, start: u32, count: u32) -> Result<(), Error> {
        let remaining = self.steps_left;
        if count as usize > remaining {
            let span = chunk.tick_spans[start as usize + remaining];
            return self.step_limit_result(span);
        }
        self.steps_left -= count as usize;
        Ok(())
    }

    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(super) fn try_charge(&mut self, amount: usize) -> bool {
        try_charge_steps(&mut self.steps_left, amount)
    }

    #[cold]
    pub(super) fn step_limit_err(&self, span: Span) -> Error {
        step_limit_error(self.limits.steps, span)
    }

    // 大きな Error の一時値を呼び出し側の通常フレームへ置かない。
    #[cold]
    #[inline(never)]
    pub(super) fn step_limit_result<T>(&self, span: Span) -> Result<T, Error> {
        Err(step_limit_error(self.limits.steps, span))
    }

    #[cold]
    #[inline(never)]
    pub(super) fn work_oom(&self, span: Span) -> Error {
        Error::new(
            "eval.limits",
            ErrorKind::TooMuchMemory,
            span,
            format!("memory limit exceeded ({} bytes)", self.pool.core.max_bytes()),
        )
    }

    #[inline(always)]
    pub(super) fn stack_push(&mut self, val: RVal, span: Span) -> Result<(), Error> {
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
    pub(super) fn stack_resize_void(&mut self, new_len: usize, span: Span) -> Result<(), Error> {
        if self
            .stack
            .resize_work(&mut self.pool.work, &mut self.pool.core, new_len, RVal::Void)
            .is_none()
        {
            return Err(self.work_oom(span));
        }
        Ok(())
    }

    #[inline(always)]
    pub(super) fn stack_reserve(&mut self, additional: usize, span: Span) -> Result<(), Error> {
        if self
            .stack
            .reserve_work(&mut self.pool.work, &mut self.pool.core, additional)
            .is_none()
        {
            return Err(self.work_oom(span));
        }
        Ok(())
    }

    pub(super) fn enter_depth(&mut self, span: Span) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > self.limits.depth {
            return Err(self.depth_limit_err(span));
        }
        Ok(())
    }

    #[cold]
    pub(super) fn depth_limit_err(&self, span: Span) -> Error {
        Error::new(
            "eval.limits",
            ErrorKind::TooDeep,
            span,
            format!("call depth limit exceeded ({})", self.limits.depth),
        )
    }

    pub(super) fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub(super) fn lookup_global(
        &mut self,
        id: u32,
        span: Span,
        chunk: &Chunk,
        ip: usize,
    ) -> Result<RVal, Error> {
        let idx = id as usize;
        if let Some(mod_id) = self.active_module {
            if let Some(Some(v)) = self.module_envs[mod_id as usize].get(idx).copied() {
                self.pool.retain_val(v);
                return Ok(v);
            }
        } else if let Some(bindings) = self.eval_bindings
            && let Some(v) = bindings.values.get(id)
        {
            self.pool.retain_val(v);
            return Ok(v);
        }
        if let Some(Some(v)) = self.globals.get(idx).copied() {
            self.pool.retain_val(v);
            return Ok(v);
        }
        if self.frozen
            && self.active_module.is_none()
            && let Some(name) = self.syms.names.get(idx)
            && let Some(&v) = self.modules.get(name.as_ref())
        {
            self.pool.retain_val(v);
            return Ok(v);
        }
        Err(self.unknown_name(id, span, chunk, ip))
    }

    #[cold]
    fn unknown_name(&self, id: u32, span: Span, chunk: &Chunk, ip: usize) -> Error {
        let name = self
            .syms
            .names
            .get(id as usize)
            .map(|s| s.as_ref())
            .unwrap_or("?");
        let eval_names = self
            .active_module
            .is_none()
            .then_some(self.eval_bindings)
            .flatten()
            .into_iter()
            .flat_map(|b| b.raw.iter().map(|(k, _)| *k));
        let frozen_mod_names = (self.frozen && self.active_module.is_none())
            .then(|| self.modules.keys().map(String::as_str))
            .into_iter()
            .flatten();
        Error::runtime(
            "expr.name",
            span,
            suggest(
                format!("unknown name '{name}'"),
                name,
                chunk
                    .visible_names(ip)
                    .chain(eval_names)
                    .chain(bound_names(self.syms, self.globals))
                    .chain(frozen_mod_names)
                    .chain(
                        self.active_module
                            .into_iter()
                            .flat_map(|id| bound_names(self.syms, &self.module_envs[id as usize])),
                    ),
            ),
        )
    }

    pub(super) fn define_top(&mut self, id: u32, val: RVal) {
        let prev = if let Some(mod_id) = self.active_module {
            set_slot(&mut self.module_envs[mod_id as usize], id, val)
        } else {
            set_slot(self.globals, id, val)
        };
        if let Some(p) = prev {
            self.pool.release_val(p);
        }
    }

    pub(super) fn release_all(&mut self, vals: impl IntoIterator<Item = RVal>) {
        for v in vals {
            self.pool.release_val(v);
        }
    }

    pub(super) fn release_slice(&mut self, vals: &[RVal]) {
        for &v in vals {
            self.pool.release_val(v);
        }
    }

    pub(super) fn release_stack_from(&mut self, from: usize) {
        while self.stack.len() > from {
            let v = self.stack.pop().unwrap();
            self.pool.release_val(v);
        }
    }

    pub(super) fn build_closure(
        &mut self,
        proto: &Rc<FnProto>,
        captures: &[RVal],
        base: usize,
        span: Span,
    ) -> Result<RVal, Error> {
        if proto.captures.is_empty() {
            let id = self
                .pool
                .alloc_closure_owned(Rc::clone(proto), &[], span)?;
            return Ok(RVal::Obj(id));
        }
        let mark = self.pool.work.mark(&self.pool.core);
        let mut cap_vals = RawVec::<RVal>::new();
        if cap_vals
            .reserve_work(&mut self.pool.work, &mut self.pool.core, proto.captures.len())
            .is_none()
        {
            self.pool.work.rewind(&mut self.pool.core, mark);
            return Err(self.work_oom(span));
        }
        for cap in &proto.captures {
            let v = match cap {
                Capture::Local(slot) => self.stack.as_slice()[base + *slot as usize],
                Capture::Upvalue(idx) => captures[*idx as usize],
            };
            self.pool.retain_val(v);
            let _ = cap_vals.push_work(&mut self.pool.work, &mut self.pool.core, v);
        }
        let res = self
            .pool
            .alloc_closure_owned(Rc::clone(proto), cap_vals.as_slice(), span);
        self.pool.work.rewind(&mut self.pool.core, mark);
        Ok(RVal::Obj(res?))
    }

    pub(super) fn run_top_chunk(&mut self, chunk: &Chunk) -> Result<RVal, Error> {
        let base = self.stack.len();
        self.stack_resize_void(base + chunk.local_count as usize, Span::default())?;
        let frame = super::vm::RinFrame {
            entry: FrameSource::Top(chunk),
            tail_source: None,
            tail_calls: 0,
            captures: ObjSlice::empty(),
            base,
            frame_stack_start: base + chunk.local_count as usize,
            ip: 0,
            origin: None,
            caller_module: self.active_module,
            prev_active_module: self.active_module,
            cleanup_stack_to: base,
            counted_depth: false,
        };
        self.run_vm(frame)
    }

    #[cold]
    #[inline(never)]
    pub(super) fn call_type_error(&self, callee: RVal, span: Span) -> Error {
        Error::runtime(
            "expr.call",
            span,
            format!("cannot call value of type {}", callee.type_name(self.pool)),
        )
    }

    #[cold]
    #[inline(never)]
    pub(super) fn call_arity_error(&self, expected: u32, actual: usize, span: Span) -> Error {
        Error::runtime(
            "expr.call",
            span,
            format!("expected {expected} arguments, got {actual}"),
        )
    }

    pub(super) fn apply(
        &mut self,
        callee: RVal,
        args: &[RVal],
        span: Span,
        origin: CallOrigin<'_>,
    ) -> Result<RVal, Error> {
        match callee {
            RVal::Obj(id) => {
                let (proto, captures) = match self.pool.get(id) {
                    Obj::Closure { proto, captures } => (Rc::clone(proto), *captures),
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
                    let _ = self.stack.push_work(&mut self.pool.work, &mut self.pool.core, val);
                }
                let _ = self
                    .stack
                    .resize_work(&mut self.pool.work, &mut self.pool.core, base + local_count, RVal::Void);
                let frame = super::vm::RinFrame {
                    entry: FrameSource::Proto(proto),
                    tail_source: None,
                    tail_calls: 0,
                    captures,
                    base,
                    frame_stack_start: base + local_count,
                    ip: 0,
                    origin: Some(origin),
                    caller_module: prev_mod,
                    prev_active_module: prev_mod,
                    cleanup_stack_to: base,
                    counted_depth: true,
                };
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

#[cold]
#[inline(never)]
fn step_limit_error(limit: usize, span: Span) -> Error {
    Error::new(
        "eval.limits",
        ErrorKind::TooManySteps,
        span,
        format!("step limit exceeded ({limit})"),
    )
}
