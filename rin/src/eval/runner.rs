//! 評価ごとの状態操作、資源制限、束縛解決と呼び出し。
use super::*;

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

    // exec_frame へ展開するとレジスタ退避が増えて末尾呼び出しループが遅くなるため分離する。
    #[inline(never)]
    pub(super) fn lookup_global(&mut self, id: u32) -> Option<RVal> {
        let idx = id as usize;
        if let Some(mod_id) = self.active_module {
            if let Some(Some(v)) = self.module_envs[mod_id as usize].get(idx).copied() {
                self.pool.retain_val(v);
                return Some(v);
            }
        } else if let Some(bindings) = self.eval_bindings {
            for &(k, v) in bindings.entries {
                if k == id {
                    self.pool.retain_val(v);
                    return Some(v);
                }
            }
        }
        if let Some(Some(v)) = self.globals.get(idx).copied() {
            self.pool.retain_val(v);
            return Some(v);
        }
        if self.active_module.is_none()
            && let Some(Some(v)) = self.frozen_modules.get(idx).copied()
        {
            self.pool.retain_val(v);
            return Some(v);
        }
        None
    }

    #[cold]
    #[inline(never)]
    pub(super) fn exec_def_global(&mut self, id: u32, span: Span) -> Result<(), Error> {
        let v = self.stack.pop().expect("stack underflow on DefGlobal");
        if self.frozen {
            self.pool.release_val(v);
            return Err(Error::runtime(
                "env.frozen",
                span,
                "'def' is not allowed in a frozen environment",
            ));
        }
        self.define_top(id, v);
        self.stack.push(RVal::Void);
        Ok(())
    }

    #[cold]
    #[inline(never)]
    pub(super) fn exec_def_fn(
        &mut self,
        name_id: u32,
        proto: &Rc<FnProto>,
        captures: &[RVal],
        base: usize,
        span: Span,
    ) -> Result<(), Error> {
        if self.frozen {
            return Err(Error::runtime(
                "env.frozen",
                span,
                "'defn' is not allowed in a frozen environment",
            ));
        }
        let closure = self.build_closure(proto, captures, base, span)?;
        self.define_top(name_id, closure);
        self.stack.push(RVal::Void);
        Ok(())
    }

    #[cold]
    #[inline(never)]
    pub(super) fn exec_import(&mut self, id: u32, span: Span) -> Result<(), Error> {
        if self.frozen {
            return Err(Error::runtime(
                "env.frozen",
                span,
                "'import' is not allowed in a frozen environment",
            ));
        }
        let name = &self.syms.names[id as usize];
        let Some(&mod_val) = self.modules.get(name.as_ref()) else {
            return Err(Error::runtime(
                "expr.module",
                span,
                suggest(
                    format!("unknown module '{name}'"),
                    name,
                    self.modules.keys().map(String::as_str),
                ),
            ));
        };
        self.pool.retain_val(mod_val);
        self.define_top(id, mod_val);
        self.stack.push(RVal::Void);
        Ok(())
    }

    #[cold]
    #[inline(never)]
    pub(super) fn exec_export(&mut self, names: &[String], span: Span) -> Result<(), Error> {
        if self.frozen {
            return Err(Error::runtime(
                "env.frozen",
                span,
                "'export' is not allowed in a frozen environment",
            ));
        }
        if self.active_module.is_none() {
            return Err(Error::runtime(
                "expr.module",
                span,
                "'export' can only be used inside a module",
            ));
        }
        let list = self.export_list.get_or_insert_with(Vec::new);
        list.extend(names.iter().cloned());
        self.stack.push(RVal::Void);
        Ok(())
    }

    #[cold]
    #[inline(never)]
    pub(super) fn unknown_name(&self, id: u32, span: Span, chunk: &Chunk, ip: usize) -> Error {
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
        let cap_vals: Rc<[RVal]> = proto
            .captures
            .iter()
            .map(|cap| {
                let v = match cap {
                    Capture::Local(slot) => self.stack[base + *slot as usize],
                    Capture::Upvalue(idx) => captures[*idx as usize],
                };
                self.pool.retain_val(v);
                v
            })
            .collect();
        let id = self.pool.alloc_obj(
            Obj::Closure {
                proto: Rc::clone(proto),
                captures: cap_vals,
            },
            span,
        )?;
        Ok(RVal::Obj(id))
    }

    pub(super) fn run_top_chunk(&mut self, chunk: &Chunk) -> Result<RVal, Error> {
        let base = self.stack.len();
        self.stack
            .resize(base + chunk.local_count as usize, RVal::Void);
        let res = self.exec_frame(
            FrameSource::Top(chunk),
            Rc::from([]),
            base,
            chunk.local_count as usize,
            None,
            self.active_module,
        );
        match res {
            Ok(val) => {
                debug_assert_eq!(self.stack.len(), base + chunk.local_count as usize);
                self.release_stack_from(base);
                Ok(val)
            }
            Err(e) => {
                self.release_stack_from(base);
                Err(e)
            }
        }
    }

    #[cfg(debug_assertions)]
    #[cold]
    #[inline(never)]
    pub(super) fn call_type_error(&self, callee: RVal, span: Span) -> Error {
        Error::runtime(
            "expr.call",
            span,
            format!("cannot call value of type {}", callee.type_name(self.pool)),
        )
    }

    #[cfg(debug_assertions)]
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
                    Obj::Closure { proto, captures } => (Rc::clone(proto), Rc::clone(captures)),
                    _ => {
                        #[cfg(debug_assertions)]
                        return Err(self.call_type_error(callee, span));
                        #[cfg(not(debug_assertions))]
                        return Err(Error::runtime(
                            "expr.call",
                            span,
                            format!("cannot call value of type {}", callee.type_name(self.pool)),
                        ));
                    }
                };
                if proto.arity as usize != args.len() {
                    #[cfg(debug_assertions)]
                    return Err(self.call_arity_error(proto.arity, args.len(), span));
                    #[cfg(not(debug_assertions))]
                    return Err(Error::runtime(
                        "expr.call",
                        span,
                        format!("expected {} arguments, got {}", proto.arity, args.len()),
                    ));
                }
                self.enter_depth(span)?;
                let prev_mod = std::mem::replace(&mut self.active_module, proto.module);
                let base = self.stack.len();
                let local_count = proto.local_count as usize;
                self.stack.reserve(local_count);
                for &val in args {
                    self.pool.retain_val(val);
                    self.stack.push(val);
                }
                self.stack.resize(base + local_count, RVal::Void);
                let res = self.exec_frame(
                    FrameSource::Proto(proto),
                    captures,
                    base,
                    local_count,
                    Some(origin),
                    prev_mod,
                );
                self.active_module = prev_mod;
                self.leave_depth();
                match res {
                    Ok(val) => {
                        self.release_stack_from(base);
                        Ok(val)
                    }
                    Err(e) => {
                        self.release_stack_from(base);
                        Err(e)
                    }
                }
            }
            RVal::Builtin(b) => self.dispatch_builtin(b, args, span),
            #[cfg(debug_assertions)]
            other => Err(self.call_type_error(other, span)),
            #[cfg(not(debug_assertions))]
            other => Err(Error::runtime(
                "expr.call",
                span,
                format!("cannot call value of type {}", other.type_name(self.pool)),
            )),
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
