//! 式を明示的な作業スタックへ展開する（`WorkArena` 完結）。
use super::*;

impl<'a, 'p> Compiler<'a, 'p> {
    pub(super) fn step_expr(
        &mut self,
        expr: &'a Expr,
        is_tail: bool,
        steps: &mut RawVec<Step<'a>>,
    ) -> Result<(), Failed> {
        let span = expr.span;
        self.push_tick_span(span)?;

        match &expr.kind {
            ExprKind::Int(n) => {
                self.emit_raw(Op::Int(*n), span)?;
            }
            ExprKind::Float(x) => {
                let idx = self.add_float(*x, span)?;
                self.emit_raw(Op::Float(idx), span)?;
            }
            ExprKind::Bool(b) => {
                self.emit_raw(Op::Bool(*b), span)?;
            }
            ExprKind::Char(c) => {
                self.emit_raw(Op::Char(*c), span)?;
            }
            ExprKind::Str(s) => {
                let idx = self.add_string(s.as_str(), span)?;
                self.emit(Op::Str(idx), span)?;
            }
            ExprKind::Template(parts) => {
                let parts = parts.as_slice();
                if u32::try_from(parts.len()).is_err() {
                    return Err(self.fail_parse(
                        "compile.capacity",
                        span,
                        "template literal too large",
                    ));
                }
                let mut part_spans = RawVec::new();
                for part in parts {
                    if part_spans
                        .push_work(&mut self.pool.work, &mut self.pool.core, part.span)
                        .is_none()
                    {
                        return Err(self.fail_oom(part.span));
                    }
                }
                let region = self.current_region();
                let top = self.scopes.last_mut().unwrap();
                let Some(tmpl_idx) = top.chunk.add_template_spans(self.pool, region, part_spans)
                else {
                    return Err(self.fail_oom(span));
                };
                self.push_step(
                    steps,
                    Step::EmitFlushed {
                        op: Op::FormatTemplate(tmpl_idx),
                        span,
                    },
                    span,
                )?;
                for part in parts.iter().rev() {
                    self.push_step(
                        steps,
                        Step::Expr {
                            expr: part,
                            is_tail: false,
                        },
                        part.span,
                    )?;
                }
            }
            ExprKind::None => {
                self.emit_raw(Op::None, span)?;
            }
            ExprKind::Ident(name) => {
                let op = self.resolve_var(*name, span)?;
                match op {
                    Op::GetGlobal(_) => {
                        self.emit(op, span)?;
                    }
                    _ => {
                        self.emit_raw(op, span)?;
                    }
                }
            }
            ExprKind::Quote(datum) | ExprKind::Quasiquote(datum) => {
                self.push_step(steps, Step::Datum(datum.as_ref()), span)?;
            }
            ExprKind::List(items) => {
                let items = items.as_slice();
                let Ok(count) = u32::try_from(items.len()) else {
                    return Err(self.fail_parse(
                        "compile.capacity",
                        span,
                        "list literal too large",
                    ));
                };
                if count == 0 {
                    self.emit(Op::EmptyList, span)?;
                } else {
                    self.push_step(
                        steps,
                        Step::EmitFlushed {
                            op: Op::MakeList(count),
                            span,
                        },
                        span,
                    )?;
                    for item in items.iter().rev() {
                        self.push_step(
                            steps,
                            Step::Expr {
                                expr: item,
                                is_tail: false,
                            },
                            item.span,
                        )?;
                    }
                }
            }
            ExprKind::Tuple(items) => {
                let items = items.as_slice();
                let Ok(count) = u32::try_from(items.len()) else {
                    return Err(self.fail_parse(
                        "compile.capacity",
                        span,
                        "tuple literal too large",
                    ));
                };
                self.push_step(
                    steps,
                    Step::EmitFlushed {
                        op: Op::MakeTuple(count),
                        span,
                    },
                    span,
                )?;
                for item in items.iter().rev() {
                    self.push_step(
                        steps,
                        Step::Expr {
                            expr: item,
                            is_tail: false,
                        },
                        item.span,
                    )?;
                }
            }
            ExprKind::Struct(s) => {
                let s = s.as_ref();
                let fields = s.fields.as_slice();
                let field_names = self.build_field_names(fields, span)?;
                let step_box = self.alloc_step_box(
                    FinishStructStep {
                        name: s.name,
                        field_names,
                        span,
                    },
                    span,
                )?;
                self.push_step(steps, Step::FinishStruct(step_box), span)?;
                for (_, fexpr) in fields.iter().rev() {
                    self.push_step(
                        steps,
                        Step::Expr {
                            expr: fexpr,
                            is_tail: false,
                        },
                        fexpr.span,
                    )?;
                }
            }
            ExprKind::With { target, fields } => {
                let fields = fields.as_slice();
                let field_names = self.build_field_names(fields, span)?;
                self.push_step(steps, Step::FinishWith { field_names, span }, span)?;
                for (idx, (fname, fexpr)) in fields.iter().enumerate().rev() {
                    let f_idx = self.add_string(fname.as_str(), span)?;
                    self.push_step(
                        steps,
                        Step::Expr {
                            expr: fexpr,
                            is_tail: false,
                        },
                        fexpr.span,
                    )?;
                    self.push_step(
                        steps,
                        Step::EmitFlushed {
                            op: Op::WithCheckField(f_idx, idx as u32),
                            span,
                        },
                        span,
                    )?;
                }
                self.push_step(
                    steps,
                    Step::EmitFlushed {
                        op: Op::WithCheckStruct,
                        span,
                    },
                    span,
                )?;
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: target.as_ref(),
                        is_tail: false,
                    },
                    target.span,
                )?;
            }
            ExprKind::Variant(v) => {
                let v = v.as_ref();
                let t_idx = self.add_string(v.type_name.as_str(), span)?;
                let v_idx = self.add_string(v.variant.as_str(), span)?;
                let op = Op::MakeVariant {
                    type_name: t_idx,
                    variant: v_idx,
                    has_data: v.data.is_some(),
                };
                if let Some(d) = &v.data {
                    self.push_step(steps, Step::EmitFlushed { op, span }, span)?;
                    self.push_step(
                        steps,
                        Step::Expr {
                            expr: d.as_ref(),
                            is_tail: false,
                        },
                        d.span,
                    )?;
                } else {
                    self.emit(op, span)?;
                }
            }
            ExprKind::Some(inner) => {
                self.push_step(
                    steps,
                    Step::EmitFlushed {
                        op: Op::MakeSome,
                        span,
                    },
                    span,
                )?;
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: inner.as_ref(),
                        is_tail: false,
                    },
                    inner.span,
                )?;
            }
            ExprKind::Ok(inner) => {
                self.push_step(
                    steps,
                    Step::EmitFlushed {
                        op: Op::MakeOk,
                        span,
                    },
                    span,
                )?;
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: inner.as_ref(),
                        is_tail: false,
                    },
                    inner.span,
                )?;
            }
            ExprKind::Err(inner) => {
                self.push_step(
                    steps,
                    Step::EmitFlushed {
                        op: Op::MakeErr,
                        span,
                    },
                    span,
                )?;
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: inner.as_ref(),
                        is_tail: false,
                    },
                    inner.span,
                )?;
            }
            ExprKind::Field { target, field } => {
                let f_idx = self.add_string(field.as_str(), span)?;
                self.push_step(
                    steps,
                    Step::EmitFlushed {
                        op: Op::GetField(f_idx),
                        span,
                    },
                    span,
                )?;
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: target.as_ref(),
                        is_tail: false,
                    },
                    target.span,
                )?;
            }
            ExprKind::Index { target, index } => {
                self.push_step(
                    steps,
                    Step::EmitFlushed {
                        op: Op::GetIndex,
                        span,
                    },
                    span,
                )?;
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: index.as_ref(),
                        is_tail: false,
                    },
                    index.span,
                )?;
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: target.as_ref(),
                        is_tail: false,
                    },
                    target.span,
                )?;
            }
            ExprKind::Def(_) => {
                return Err(self.fail_decl("def", span));
            }
            ExprKind::DefSchema(_) => {
                return Err(self.fail_decl("defschema", span));
            }
            ExprKind::Defn(_) => {
                return Err(self.fail_decl("defn", span));
            }
            ExprKind::Fn(f) => {
                let f = f.as_ref();
                let params = f.params.as_slice();
                let Ok(arity) = u32::try_from(params.len()) else {
                    return Err(self.fail_parse("compile.capacity", span, "too many parameters"));
                };
                let mut inner = self.new_scope();
                for p in params {
                    inner.alloc_slot(self.pool, p.name, span)?;
                }
                self.push_scope(inner, span)?;
                let step_box = self.alloc_step_box(FinishFnStep { arity, span }, span)?;
                self.push_step(steps, Step::FinishFn(step_box), span)?;
                self.push_body(steps, f.body.as_slice(), true, span)?;
            }
            ExprKind::Let(l) => {
                let l = l.as_ref();
                let saved_locals_len = self.scopes.last().unwrap().locals.len();
                let saved_next_slot = self.scopes.last().unwrap().next_slot;
                self.push_step(
                    steps,
                    Step::RestoreLocals {
                        saved_locals_len,
                        saved_next_slot,
                    },
                    span,
                )?;
                self.push_body(steps, l.body.as_slice(), is_tail, span)?;
                for &(name, ref val_expr) in l.bindings.as_slice().iter().rev() {
                    self.push_step(
                        steps,
                        Step::BindLocal {
                            name,
                            span: val_expr.span,
                        },
                        val_expr.span,
                    )?;
                    self.push_step(
                        steps,
                        Step::Expr {
                            expr: val_expr,
                            is_tail: false,
                        },
                        val_expr.span,
                    )?;
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let step_box = self.alloc_step_box(
                    IfAfterCondStep {
                        then_branch: then_branch.as_ref(),
                        else_branch: else_branch.as_ref(),
                        is_tail,
                        cond_span: cond.span,
                        span,
                    },
                    span,
                )?;
                self.push_step(steps, Step::IfAfterCond(step_box), span)?;
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: cond.as_ref(),
                        is_tail: false,
                    },
                    cond.span,
                )?;
            }
            ExprKind::When { cond, body } => {
                let step_box = self.alloc_step_box(
                    WhenAfterCondStep {
                        body: body.as_slice(),
                        cond_span: cond.span,
                        span,
                    },
                    span,
                )?;
                self.push_step(steps, Step::WhenAfterCond(step_box), span)?;
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: cond.as_ref(),
                        is_tail: false,
                    },
                    cond.span,
                )?;
            }
            ExprKind::Match { scrutinee, arms } => {
                let arms = arms.as_slice();
                let step_box = self.alloc_step_box(
                    MatchArmsStep {
                        arms,
                        idx: 0,
                        end_jumps: RawVec::new(),
                        is_tail,
                        span,
                    },
                    span,
                )?;
                self.push_step(steps, Step::MatchStartArms(step_box), span)?;
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: scrutinee.as_ref(),
                        is_tail: false,
                    },
                    scrutinee.span,
                )?;
            }
            ExprKind::Call { callee, args } => {
                let args = args.as_slice();
                if let ExprKind::Ident(name) = &callee.kind
                    && matches!(name.as_str(), "check" | "check_eq")
                {
                    self.push_step(
                        steps,
                        Step::EmitFlushed {
                            op: Op::Check(name.as_str() == "check_eq"),
                            span,
                        },
                        span,
                    )?;
                    for arg in args.iter().rev() {
                        self.push_step(
                            steps,
                            Step::Expr {
                                expr: arg,
                                is_tail: false,
                            },
                            arg.span,
                        )?;
                    }
                    return Ok(());
                }
                if let ExprKind::Ident(op) = &callee.kind
                    && (op.as_str() == "and" || op.as_str() == "or")
                {
                    let is_and = op.as_str() == "and";
                    if args.is_empty() {
                        self.emit(if is_and { Op::And } else { Op::Or }, span)?;
                    } else {
                        let step_box = self.alloc_step_box(
                            ShortCircuitStep {
                                is_and,
                                args,
                                idx: 0,
                                jumps: RawVec::new(),
                                span,
                            },
                            span,
                        )?;
                        self.push_step(steps, Step::ShortCircuitAfterArg(step_box), span)?;
                        self.push_step(
                            steps,
                            Step::Expr {
                                expr: &args[0],
                                is_tail: false,
                            },
                            args[0].span,
                        )?;
                    }
                    return Ok(());
                }
                let Ok(argc) = u32::try_from(args.len()) else {
                    return Err(self.fail_parse("compile.capacity", span, "too many arguments"));
                };
                let call_op = if is_tail {
                    Op::TailCall(argc)
                } else {
                    Op::Call(argc)
                };
                self.push_step(steps, Step::EmitFlushed { op: call_op, span }, span)?;
                for arg in args.iter().rev() {
                    self.push_step(
                        steps,
                        Step::Expr {
                            expr: arg,
                            is_tail: false,
                        },
                        arg.span,
                    )?;
                }
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: callee.as_ref(),
                        is_tail: false,
                    },
                    callee.span,
                )?;
            }
            ExprKind::Import { .. } => {
                return Err(self.fail_decl("import", span));
            }
            ExprKind::Export { .. } => {
                return Err(self.fail_decl("export", span));
            }
        }
        Ok(())
    }

    pub(super) fn step_datum(
        &mut self,
        datum: &'a Datum,
        steps: &mut RawVec<Step<'a>>,
    ) -> Result<(), Failed> {
        let span = datum.span;
        self.push_tick_span(span)?;
        match &datum.kind {
            DatumKind::Int(n) => {
                self.emit_raw(Op::Int(*n), span)?;
            }
            DatumKind::Float(x) => {
                let idx = self.add_float(*x, span)?;
                self.emit_raw(Op::Float(idx), span)?;
            }
            DatumKind::Bool(b) => {
                self.emit_raw(Op::Bool(*b), span)?;
            }
            DatumKind::Char(c) => {
                self.emit_raw(Op::Char(*c), span)?;
            }
            DatumKind::Str(s) => {
                let idx = self.add_string(s.as_str(), span)?;
                self.emit(Op::Str(idx), span)?;
            }
            DatumKind::None => {
                self.emit_raw(Op::None, span)?;
            }
            DatumKind::Ident(s) => {
                let idx = self.add_string(s.as_str(), span)?;
                self.emit(Op::Ident(idx), span)?;
            }
            DatumKind::Unquote(expr) | DatumKind::UnquoteSplicing(expr) => {
                self.push_step(
                    steps,
                    Step::Expr {
                        expr: expr.as_ref(),
                        is_tail: false,
                    },
                    expr.span,
                )?;
            }
            DatumKind::List(items) => {
                let items = items.as_slice();
                let Ok(count) = u32::try_from(items.len()) else {
                    return Err(self.fail_parse(
                        "compile.capacity",
                        span,
                        "quoted list literal too large",
                    ));
                };
                if count == 0 {
                    self.emit(Op::EmptyList, span)?;
                } else {
                    let has_splice = items
                        .iter()
                        .any(|d| matches!(d.kind, DatumKind::UnquoteSplicing(_)));
                    let op = if has_splice {
                        let region = self.current_region();
                        let mut mask = RawVec::new();
                        for item in items {
                            let flag = u8::from(matches!(item.kind, DatumKind::UnquoteSplicing(_)));
                            if self.pool.push_code(region, &mut mask, flag).is_none() {
                                return Err(self.fail_oom(span));
                            }
                        }
                        let mask_idx = self.add_splice_mask(mask, span)?;
                        Op::MakeListSplice(mask_idx)
                    } else {
                        Op::MakeList(count)
                    };
                    self.push_step(steps, Step::EmitFlushed { op, span }, span)?;
                    for item in items.iter().rev() {
                        self.push_step(steps, Step::Datum(item), item.span)?;
                    }
                }
            }
        }
        Ok(())
    }
}
