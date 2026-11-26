//! 式を明示的な作業スタックへ展開する。
use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn step_expr(
        &mut self,
        expr: &'a Expr,
        is_tail: bool,
        steps: &mut Vec<Step<'a>>,
    ) -> Result<(), Error> {
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
                    steps.push(Step::EmitFlushed {
                        op: Op::MakeList(count),
                        span,
                    });
                    for item in items.iter().rev() {
                        steps.push(Step::Expr {
                            expr: item,
                            is_tail: false,
                        });
                    }
                }
            }
            ExprKind::Tuple(items) => {
                let items = items.as_slice();
                let count = u32::try_from(items.len()).map_err(|_| {
                    Error::parse("compile.capacity", span, "tuple literal too large")
                })?;
                steps.push(Step::EmitFlushed {
                    op: Op::MakeTuple(count),
                    span,
                });
                for item in items.iter().rev() {
                    steps.push(Step::Expr {
                        expr: item,
                        is_tail: false,
                    });
                }
            }
            ExprKind::Struct(s) => {
                let fields = s.fields.as_slice();
                let field_names: Vec<Rc<str>> =
                    fields.iter().map(|(k, _)| Rc::from(k.as_str())).collect();
                steps.push(Step::FinishStruct(Box::new(FinishStructStep {
                    name: s.name.as_str(),
                    field_names,
                    span,
                })));
                for (_, fexpr) in fields.iter().rev() {
                    steps.push(Step::Expr {
                        expr: fexpr,
                        is_tail: false,
                    });
                }
            }
            ExprKind::With { target, fields } => {
                let fields = fields.as_slice();
                let field_names: Vec<Rc<str>> =
                    fields.iter().map(|(k, _)| Rc::from(k.as_str())).collect();
                steps.push(Step::FinishWith { field_names, span });
                for (idx, (fname, fexpr)) in fields.iter().enumerate().rev() {
                    let f_idx = self.chunk().add_string(fname.as_str());
                    steps.push(Step::Expr {
                        expr: fexpr,
                        is_tail: false,
                    });
                    steps.push(Step::EmitFlushed {
                        op: Op::WithCheckField(f_idx, idx as u32),
                        span,
                    });
                }
                steps.push(Step::EmitFlushed {
                    op: Op::WithCheckStruct,
                    span,
                });
                steps.push(Step::Expr {
                    expr: target.as_ref(),
                    is_tail: false,
                });
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
                    steps.push(Step::EmitFlushed { op, span });
                    steps.push(Step::Expr {
                        expr: d.as_ref(),
                        is_tail: false,
                    });
                } else {
                    self.emit(op, span);
                }
            }
            ExprKind::Some(inner) => {
                steps.push(Step::EmitFlushed {
                    op: Op::MakeSome,
                    span,
                });
                steps.push(Step::Expr {
                    expr: inner.as_ref(),
                    is_tail: false,
                });
            }
            ExprKind::Ok(inner) => {
                steps.push(Step::EmitFlushed {
                    op: Op::MakeOk,
                    span,
                });
                steps.push(Step::Expr {
                    expr: inner.as_ref(),
                    is_tail: false,
                });
            }
            ExprKind::Err(inner) => {
                steps.push(Step::EmitFlushed {
                    op: Op::MakeErr,
                    span,
                });
                steps.push(Step::Expr {
                    expr: inner.as_ref(),
                    is_tail: false,
                });
            }
            ExprKind::Field { target, field } => {
                let f_idx = self.chunk().add_string(field.as_str());
                steps.push(Step::EmitFlushed {
                    op: Op::GetField(f_idx),
                    span,
                });
                steps.push(Step::Expr {
                    expr: target.as_ref(),
                    is_tail: false,
                });
            }
            ExprKind::Index { target, index } => {
                steps.push(Step::EmitFlushed {
                    op: Op::GetIndex,
                    span,
                });
                steps.push(Step::Expr {
                    expr: index.as_ref(),
                    is_tail: false,
                });
                steps.push(Step::Expr {
                    expr: target.as_ref(),
                    is_tail: false,
                });
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
                steps.push(Step::FinishFn(Box::new(FinishFnStep { arity, span })));
                self.push_body(steps, f.body.as_slice(), true);
            }
            ExprKind::Let(l) => {
                let saved_locals_len = self.scopes.last().unwrap().locals.len();
                let saved_next_slot = self.scopes.last().unwrap().next_slot;
                steps.push(Step::RestoreLocals {
                    saved_locals_len,
                    saved_next_slot,
                });
                self.push_body(steps, l.body.as_slice(), is_tail);
                for (name, val_expr) in l.bindings.as_slice().iter().rev() {
                    steps.push(Step::BindLocal {
                        name: name.as_str(),
                        span: val_expr.span,
                    });
                    steps.push(Step::Expr {
                        expr: val_expr,
                        is_tail: false,
                    });
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                steps.push(Step::IfAfterCond(Box::new(IfAfterCondStep {
                    then_branch: then_branch.as_ref(),
                    else_branch: else_branch.as_ref(),
                    is_tail,
                    cond_span: cond.span,
                    span,
                })));
                steps.push(Step::Expr {
                    expr: cond.as_ref(),
                    is_tail: false,
                });
            }
            ExprKind::When { cond, body } => {
                steps.push(Step::WhenAfterCond(Box::new(WhenAfterCondStep {
                    body: body.as_slice(),
                    cond_span: cond.span,
                    span,
                })));
                steps.push(Step::Expr {
                    expr: cond.as_ref(),
                    is_tail: false,
                });
            }
            ExprKind::Match { scrutinee, arms } => {
                let arms = arms.as_slice();
                steps.push(Step::MatchStartArms(Box::new(MatchArmsStep {
                    arms,
                    idx: 0,
                    end_jumps: Vec::with_capacity(arms.len()),
                    is_tail,
                    span,
                })));
                steps.push(Step::Expr {
                    expr: scrutinee.as_ref(),
                    is_tail: false,
                });
            }
            ExprKind::Call { callee, args } => {
                let args = args.as_slice();
                if let ExprKind::Ident(name) = &callee.kind
                    && matches!(name.as_str(), "check" | "check_eq")
                {
                    steps.push(Step::EmitFlushed {
                        op: Op::Check(name.as_str() == "check_eq"),
                        span,
                    });
                    for arg in args.iter().rev() {
                        steps.push(Step::Expr {
                            expr: arg,
                            is_tail: false,
                        });
                    }
                    return Ok(());
                }
                if let ExprKind::Ident(op) = &callee.kind
                    && (op.as_str() == "and" || op.as_str() == "or")
                {
                    let is_and = op.as_str() == "and";
                    if args.is_empty() {
                        self.emit(if is_and { Op::And } else { Op::Or }, span);
                    } else {
                        steps.push(Step::ShortCircuitAfterArg(Box::new(ShortCircuitStep {
                            is_and,
                            args,
                            idx: 0,
                            jumps: Vec::with_capacity(args.len()),
                            span,
                        })));
                        steps.push(Step::Expr {
                            expr: &args[0],
                            is_tail: false,
                        });
                    }
                    return Ok(());
                }
                let argc = u32::try_from(args.len())
                    .map_err(|_| Error::parse("compile.capacity", span, "too many arguments"))?;
                let call_op = if is_tail {
                    Op::TailCall(argc)
                } else {
                    Op::Call(argc)
                };
                steps.push(Step::EmitFlushed { op: call_op, span });
                for arg in args.iter().rev() {
                    steps.push(Step::Expr {
                        expr: arg,
                        is_tail: false,
                    });
                }
                steps.push(Step::Expr {
                    expr: callee.as_ref(),
                    is_tail: false,
                });
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
