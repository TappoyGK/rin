//! 浅い構文の高速経路。深さ上限で反復処理へ切り替える。
use super::*;

impl<'a, 'p> Compiler<'a, 'p> {
    pub(super) fn compile_top_level(&mut self, exprs: &'a [Expr]) -> Result<(), Error> {
        for (i, expr) in exprs.iter().enumerate() {
            let last = i + 1 == exprs.len();
            let span = expr.span;
            match &expr.kind {
                ExprKind::Def { name, value } => {
                    if self.frozen {
                        return Err(self.decl_error("def", span));
                    }
                    self.scopes.last_mut().unwrap().chunk.tick_spans.push(span);
                    self.compile_expr_shallow(value.as_ref(), false, 1)?;
                    let n_idx = self.intern_sym(name.as_str(), span)?;
                    self.emit(Op::DefGlobal(n_idx), span);
                }
                ExprKind::Defn(d) => {
                    let d = d.as_ref();
                    let name = d.name;
                    let params = d.params.as_slice();
                    if self.frozen {
                        return Err(self.decl_error("defn", span));
                    }
                    self.scopes.last_mut().unwrap().chunk.tick_spans.push(span);
                    let arity = u32::try_from(params.len()).map_err(|_| {
                        Error::parse("compile.capacity", span, "too many parameters")
                    })?;
                    let mut inner = self.new_scope();
                    for p in params {
                        inner.alloc_slot(p.as_str(), span)?;
                    }
                    self.scopes.push(inner);
                    self.compile_body_shallow(d.body.as_slice(), true, 1)?;
                    let finished = self.scopes.pop().unwrap();
                    let captures: Vec<Capture> =
                        finished.captures.into_iter().map(|(_, c)| c).collect();
                    let proto = Rc::new(FnProto {
                        name: Some(name.as_str().to_owned()),
                        module: finished.module,
                        arity,
                        local_count: finished.max_slots,
                        captures,
                        chunk: finished.chunk,
                    });
                    let p_idx = self.chunk().add_proto(proto);
                    let n_idx = self.intern_sym(name.as_str(), span)?;
                    self.emit(Op::DefFn(n_idx, p_idx), span);
                }
                ExprKind::Import { name } => {
                    if self.frozen {
                        return Err(self.decl_error("import", span));
                    }
                    self.scopes.last_mut().unwrap().chunk.tick_spans.push(span);
                    let idx = self.intern_sym(name.as_str(), span)?;
                    self.emit(Op::Import(idx), span);
                }
                ExprKind::Export { names } => {
                    if self.frozen {
                        return Err(self.decl_error("export", span));
                    }
                    self.scopes.last_mut().unwrap().chunk.tick_spans.push(span);
                    let owned_names: Vec<String> = names
                        .as_slice()
                        .iter()
                        .map(|n| n.as_str().to_owned())
                        .collect();
                    let idx = self.chunk().add_exports(owned_names);
                    self.emit(Op::Export(idx), span);
                }
                _ => self.compile_expr_shallow(expr, false, 0)?,
            }
            if !last {
                self.emit(Op::Pop, expr.span);
            }
        }
        if !exprs.is_empty() {
            self.flush_ticks();
        }
        Ok(())
    }

    pub(super) fn compile_body_shallow(
        &mut self,
        exprs: &'a [Expr],
        is_tail: bool,
        depth: usize,
    ) -> Result<(), Error> {
        for (i, expr) in exprs.iter().enumerate() {
            let last = i + 1 == exprs.len();
            self.compile_expr_shallow(expr, last && is_tail, depth)?;
            if !last {
                self.emit(Op::Pop, expr.span);
            }
        }
        if !exprs.is_empty() {
            self.flush_ticks();
        }
        Ok(())
    }

    #[cold]
    #[inline(never)]
    fn compile_expr_iter(&mut self, expr: &'a Expr, is_tail: bool) -> Result<(), Error> {
        let mut steps = Vec::with_capacity(16);
        steps.push(Step::Expr { expr, is_tail });
        self.run_steps(&mut steps)
    }

    pub(super) fn compile_expr_shallow(
        &mut self,
        expr: &'a Expr,
        is_tail: bool,
        depth: usize,
    ) -> Result<(), Error> {
        if depth >= SHALLOW_COMPILE_DEPTH {
            return self.compile_expr_iter(expr, is_tail);
        }
        let span = expr.span;
        self.scopes.last_mut().unwrap().chunk.tick_spans.push(span);

        match &expr.kind {
            ExprKind::Int(n) => {
                self.chunk().emit(Op::Int(*n), span);
            }
            ExprKind::Float(x) => {
                let idx = self.chunk().add_float(*x);
                self.chunk().emit(Op::Float(idx), span);
            }
            ExprKind::Bool(b) => {
                self.chunk().emit(Op::Bool(*b), span);
            }
            ExprKind::Char(c) => {
                self.chunk().emit(Op::Char(*c), span);
            }
            ExprKind::Str(s) => {
                let idx = self.chunk().add_string(s.as_str());
                self.emit(Op::Str(idx), span);
            }
            ExprKind::None => {
                self.chunk().emit(Op::None, span);
            }
            ExprKind::Ident(name) => {
                let op = self.resolve_var(name.as_str(), span)?;
                match op {
                    Op::GetGlobal(_) => {
                        self.emit(op, span);
                    }
                    _ => {
                        self.chunk().emit(op, span);
                    }
                }
            }
            ExprKind::List(items) => {
                let items = items.as_slice();
                let count = u32::try_from(items.len()).map_err(|_| {
                    Error::parse("compile.capacity", span, "list literal too large")
                })?;
                if count == 0 {
                    self.emit(Op::EmptyList, span);
                } else {
                    for item in items {
                        self.compile_expr_shallow(item, false, depth + 1)?;
                    }
                    self.emit(Op::MakeList(count), span);
                }
            }
            ExprKind::Tuple(items) => {
                let items = items.as_slice();
                let count = u32::try_from(items.len()).map_err(|_| {
                    Error::parse("compile.capacity", span, "tuple literal too large")
                })?;
                for item in items {
                    self.compile_expr_shallow(item, false, depth + 1)?;
                }
                self.emit(Op::MakeTuple(count), span);
            }
            ExprKind::Struct(s) => {
                let fields = s.fields.as_slice();
                let field_names: Vec<Rc<str>> =
                    fields.iter().map(|(k, _)| Rc::from(k.as_str())).collect();
                for (_, fexpr) in fields {
                    self.compile_expr_shallow(fexpr, false, depth + 1)?;
                }
                let s_idx = self.chunk().add_struct(StructDesc {
                    name: Rc::from(s.name.as_str()),
                    fields: field_names,
                });
                self.emit(Op::MakeStruct(s_idx), span);
            }
            ExprKind::With { target, fields } => {
                self.compile_expr_shallow(target.as_ref(), false, depth + 1)?;
                self.emit(Op::WithCheckStruct, span);
                let fields = fields.as_slice();
                let field_names: Vec<Rc<str>> =
                    fields.iter().map(|(k, _)| Rc::from(k.as_str())).collect();
                for (idx, (fname, fexpr)) in fields.iter().enumerate() {
                    let f_idx = self.chunk().add_string(fname.as_str());
                    self.emit(Op::WithCheckField(f_idx, idx as u32), span);
                    self.compile_expr_shallow(fexpr, false, depth + 1)?;
                }
                let s_idx = self.chunk().add_struct(StructDesc {
                    name: Rc::from(""),
                    fields: field_names,
                });
                self.emit(Op::WithFinish(s_idx), span);
            }
            ExprKind::Variant(v) => {
                let t_idx = self.chunk().add_string(v.type_name.as_str());
                let v_idx = self.chunk().add_string(v.variant.as_str());
                let op = Op::MakeVariant {
                    type_name: t_idx,
                    variant: v_idx,
                    has_data: v.data.is_some(),
                };
                if let Some(d) = &v.data {
                    self.compile_expr_shallow(d.as_ref(), false, depth + 1)?;
                }
                self.emit(op, span);
            }
            ExprKind::Some(inner) => {
                self.compile_expr_shallow(inner.as_ref(), false, depth + 1)?;
                self.emit(Op::MakeSome, span);
            }
            ExprKind::Ok(inner) => {
                self.compile_expr_shallow(inner.as_ref(), false, depth + 1)?;
                self.emit(Op::MakeOk, span);
            }
            ExprKind::Err(inner) => {
                self.compile_expr_shallow(inner.as_ref(), false, depth + 1)?;
                self.emit(Op::MakeErr, span);
            }
            ExprKind::Field { target, field } => {
                self.compile_expr_shallow(target.as_ref(), false, depth + 1)?;
                let f_idx = self.chunk().add_string(field.as_str());
                self.emit(Op::GetField(f_idx), span);
            }
            ExprKind::Index { target, index } => {
                self.compile_expr_shallow(target.as_ref(), false, depth + 1)?;
                self.compile_expr_shallow(index.as_ref(), false, depth + 1)?;
                self.emit(Op::GetIndex, span);
            }
            ExprKind::Def { .. } => {
                return Err(self.decl_error("def", span));
            }
            ExprKind::Defn(_) => {
                return Err(self.decl_error("defn", span));
            }
            ExprKind::Fn(f) => {
                let params = f.params.as_slice();
                let arity = u32::try_from(params.len())
                    .map_err(|_| Error::parse("compile.capacity", span, "too many parameters"))?;
                let mut inner = self.new_scope();
                for p in params {
                    inner.alloc_slot(p.as_str(), span)?;
                }
                self.scopes.push(inner);
                self.compile_body_shallow(f.body.as_slice(), true, depth + 1)?;
                let finished = self.scopes.pop().unwrap();
                let captures: Vec<Capture> =
                    finished.captures.into_iter().map(|(_, c)| c).collect();
                let proto = Rc::new(FnProto {
                    name: None,
                    module: finished.module,
                    arity,
                    local_count: finished.max_slots,
                    captures,
                    chunk: finished.chunk,
                });
                let p_idx = self.chunk().add_proto(proto);
                self.emit(Op::MakeClosure(p_idx), span);
            }
            ExprKind::Let(l) => {
                let saved_locals_len = self.scopes.last().unwrap().locals.len();
                let saved_next_slot = self.scopes.last().unwrap().next_slot;
                for (name, val_expr) in l.bindings.as_slice() {
                    self.compile_expr_shallow(val_expr, false, depth + 1)?;
                    let slot = self
                        .scopes
                        .last_mut()
                        .unwrap()
                        .alloc_slot(name.as_str(), val_expr.span)?;
                    self.chunk().emit(Op::SetLocal(slot), val_expr.span);
                }
                self.compile_body_shallow(l.body.as_slice(), is_tail, depth + 1)?;
                let scope = self.scopes.last_mut().unwrap();
                scope.truncate_locals(saved_locals_len);
                scope.next_slot = saved_next_slot;
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.compile_expr_shallow(cond.as_ref(), false, depth + 1)?;
                let j_else = self.emit(Op::JumpIfFalse(0), cond.span);
                self.compile_expr_shallow(then_branch.as_ref(), is_tail, depth + 1)?;
                let j_end = self.emit(Op::Jump(0), span);
                let else_ip = self.chunk().code.len();
                self.patch_jump(j_else, else_ip);
                self.compile_expr_shallow(else_branch.as_ref(), is_tail, depth + 1)?;
                self.flush_ticks();
                let end_ip = self.chunk().code.len();
                self.patch_jump(j_end, end_ip);
            }
            ExprKind::When { cond, body } => {
                self.compile_expr_shallow(cond.as_ref(), false, depth + 1)?;
                let j_end = self.emit(Op::JumpIfFalse(0), cond.span);
                self.compile_body_shallow(body.as_slice(), false, depth + 1)?;
                self.emit(Op::Pop, span);
                let end_ip = self.chunk().code.len();
                self.patch_jump(j_end, end_ip);
                self.chunk().emit(Op::Void, span);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.compile_expr_shallow(scrutinee.as_ref(), false, depth + 1)?;
                let arms = arms.as_slice();
                let mut end_jumps = Vec::with_capacity(arms.len());
                for arm in arms {
                    let pat_idx = self.chunk().add_pattern(arm.pattern.to_owned_pattern());
                    let try_ip = self.emit(
                        Op::TryMatch {
                            pat_idx,
                            fail_ip: 0,
                        },
                        arm.span,
                    );
                    let saved_locals_len = self.scopes.last().unwrap().locals.len();
                    let saved_next_slot = self.scopes.last().unwrap().next_slot;
                    if let Some(bind_name) = pattern_binding(&arm.pattern) {
                        let slot = self
                            .scopes
                            .last_mut()
                            .unwrap()
                            .alloc_slot(bind_name, arm.span)?;
                        self.chunk().emit(Op::SetLocal(slot), arm.span);
                    }
                    self.chunk().emit(Op::Pop, arm.span);
                    self.compile_body_shallow(arm.body.as_slice(), is_tail, depth + 1)?;
                    let scope = self.scopes.last_mut().unwrap();
                    scope.truncate_locals(saved_locals_len);
                    scope.next_slot = saved_next_slot;
                    end_jumps.push(self.emit(Op::Jump(0), arm.span));
                    let fail_ip = self.chunk().code.len();
                    self.patch_jump(try_ip, fail_ip);
                }
                self.chunk().emit(Op::Pop, span);
                self.emit(Op::MatchFail, span);
                let end_ip = self.chunk().code.len();
                for j in end_jumps {
                    self.patch_jump(j, end_ip);
                }
            }
            ExprKind::Call { callee, args } => {
                let args = args.as_slice();
                if let ExprKind::Ident(name) = &callee.kind
                    && matches!(name.as_str(), "check" | "check_eq")
                {
                    for arg in args {
                        self.compile_expr_shallow(arg, false, depth + 1)?;
                    }
                    self.emit(Op::Check(name.as_str() == "check_eq"), span);
                    return Ok(());
                }
                if let ExprKind::Ident(op) = &callee.kind
                    && (op.as_str() == "and" || op.as_str() == "or")
                {
                    let is_and = op.as_str() == "and";
                    if args.is_empty() {
                        self.emit(if is_and { Op::And } else { Op::Or }, span);
                    } else {
                        let mut jumps = Vec::with_capacity(args.len());
                        for arg in args {
                            self.compile_expr_shallow(arg, false, depth + 1)?;
                            let j = if is_and {
                                self.emit(Op::JumpIfFalsePeek(0), arg.span)
                            } else {
                                self.emit(Op::JumpIfTruePeek(0), arg.span)
                            };
                            jumps.push(j);
                        }
                        self.chunk().emit(Op::Bool(is_and), span);
                        let end_ip = self.chunk().code.len();
                        for jmp in jumps {
                            self.patch_jump(jmp, end_ip);
                        }
                    }
                    return Ok(());
                }
                let argc = u32::try_from(args.len())
                    .map_err(|_| Error::parse("compile.capacity", span, "too many arguments"))?;
                self.compile_expr_shallow(callee.as_ref(), false, depth + 1)?;
                for arg in args {
                    self.compile_expr_shallow(arg, false, depth + 1)?;
                }
                let call_op = if is_tail {
                    Op::TailCall(argc)
                } else {
                    Op::Call(argc)
                };
                self.emit(call_op, span);
            }
            ExprKind::Import { .. } => {
                return Err(self.decl_error("import", span));
            }
            ExprKind::Export { .. } => {
                return Err(self.decl_error("export", span));
            }
        }
        Ok(())
    }
}
