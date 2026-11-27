use super::*;

impl Parser<'_, '_, '_> {
    pub(super) fn resume_frame(
        &mut self,
        frame: ParseFrame,
        expr: Expr,
        depth: usize,
    ) -> Result<NextStep, ErrRecorded> {
        match frame {
            ParseFrame::PostfixIndex {
                target,
                target_depth,
            } => {
                self.expect(WorkTokenKind::RBracket, "']'")?;
                let combined = 1 + target_depth.max(depth);
                self.check_nesting(self.frames.len() + combined, target.span)?;
                let span = target.span;
                let index = self.alloc_box(expr, span)?;
                Ok(NextStep::HaveExpr(
                    Expr::new(span, ExprKind::Index { target, index }),
                    combined,
                ))
            }
            ParseFrame::FieldTarget { start } => {
                let (field, _) = self.expect_ident("field name")?;
                self.expect(WorkTokenKind::RParen, "')'")?;
                let target = self.alloc_box(expr, start)?;
                Ok(NextStep::HaveExpr(
                    Expr::new(start, ExprKind::Field { target, field }),
                    1 + depth,
                ))
            }
            ParseFrame::IndexTarget { start } => {
                let target = self.alloc_box(expr, start)?;
                self.push_frame(
                    ParseFrame::IndexArg {
                        start,
                        target,
                        target_depth: depth,
                    },
                    start,
                )?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::IndexArg {
                start,
                target,
                target_depth,
            } => {
                self.expect(WorkTokenKind::RParen, "')'")?;
                let combined = 1 + target_depth.max(depth);
                self.check_nesting(self.frames.len() + combined, start)?;
                let index = self.alloc_box(expr, start)?;
                Ok(NextStep::HaveExpr(
                    Expr::new(start, ExprKind::Index { target, index }),
                    combined,
                ))
            }
            ParseFrame::Template {
                start,
                mut parts,
                mut max_child,
            } => {
                if parts.push_work(self.work, self.core, expr).is_none() {
                    return Err(self.oom(expr.span));
                }
                max_child = max_child.max(depth);
                let next = self.peek();
                match next.kind {
                    WorkTokenKind::TemplateMid(s) => {
                        self.advance()?;
                        if !s.is_empty()
                            && parts
                                .push_work(
                                    self.work,
                                    self.core,
                                    Expr::new(next.span, ExprKind::Str(s)),
                                )
                                .is_none()
                        {
                            return Err(self.oom(next.span));
                        }
                        self.push_frame(
                            ParseFrame::Template {
                                start,
                                parts,
                                max_child,
                            },
                            start,
                        )?;
                        Ok(NextStep::StartExpr)
                    }
                    WorkTokenKind::TemplateEnd(s) => {
                        self.advance()?;
                        if !s.is_empty()
                            && parts
                                .push_work(
                                    self.work,
                                    self.core,
                                    Expr::new(next.span, ExprKind::Str(s)),
                                )
                                .is_none()
                        {
                            return Err(self.oom(next.span));
                        }
                        Ok(NextStep::HaveExpr(
                            Expr::new(start, ExprKind::Template(parts)),
                            1 + max_child,
                        ))
                    }
                    _ => Err(self.parse_err(
                        "src.syntax",
                        next.span,
                        "expected '}' to close template interpolation",
                    )),
                }
            }
            ParseFrame::List {
                start,
                mut items,
                mut max_child,
            } => {
                if items.push_work(self.work, self.core, expr).is_none() {
                    return Err(self.oom(expr.span));
                }
                max_child = max_child.max(depth);
                if self.peek().kind == WorkTokenKind::RBracket {
                    self.advance()?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::List(items)),
                        1 + max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(self.parse_err("src.syntax", start, "unterminated '['"));
                }
                self.push_frame(
                    ParseFrame::List {
                        start,
                        items,
                        max_child,
                    },
                    start,
                )?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::ParenFirst { start } => {
                let callee = self.alloc_box(expr, start)?;
                if self.peek().kind == WorkTokenKind::RParen {
                    self.advance()?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(
                            start,
                            ExprKind::Call {
                                callee,
                                args: RawVec::new(),
                            },
                        ),
                        1 + depth,
                    ));
                }
                if self.at_eof() {
                    return Err(self.parse_err("src.syntax", start, "unterminated '('"));
                }
                self.push_frame(
                    ParseFrame::Call {
                        start,
                        callee,
                        args: RawVec::new(),
                        max_child: depth,
                    },
                    start,
                )?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::Tuple {
                start,
                mut items,
                mut max_child,
            } => {
                if items.push_work(self.work, self.core, expr).is_none() {
                    return Err(self.oom(expr.span));
                }
                max_child = max_child.max(depth);
                if self.peek().kind == WorkTokenKind::RParen {
                    if items.len() < 2 {
                        return Err(self.parse_err(
                            "src.syntax",
                            start,
                            "tuple requires at least 2 elements",
                        ));
                    }
                    self.advance()?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::Tuple(items)),
                        1 + max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(self.parse_err("src.syntax", start, "unterminated tuple '('"));
                }
                self.push_frame(
                    ParseFrame::Tuple {
                        start,
                        items,
                        max_child,
                    },
                    start,
                )?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::Call {
                start,
                callee,
                mut args,
                mut max_child,
            } => {
                if args.push_work(self.work, self.core, expr).is_none() {
                    return Err(self.oom(expr.span));
                }
                max_child = max_child.max(depth);
                if self.peek().kind == WorkTokenKind::RParen {
                    self.advance()?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::Call { callee, args }),
                        1 + max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(self.parse_err("src.syntax", start, "unterminated '('"));
                }
                self.push_frame(
                    ParseFrame::Call {
                        start,
                        callee,
                        args,
                        max_child,
                    },
                    start,
                )?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::BuiltinWrap { span, kind } => {
                self.expect(WorkTokenKind::RParen, "')'")?;
                let boxed = self.alloc_box(expr, span)?;
                let ek = match kind {
                    WrapKind::Some => ExprKind::Some(boxed),
                    WrapKind::Ok => ExprKind::Ok(boxed),
                    WrapKind::Err => ExprKind::Err(boxed),
                };
                Ok(NextStep::HaveExpr(Expr::new(span, ek), 1 + depth))
            }
            ParseFrame::StructField(mut sf) => {
                let sf_mut = sf.as_mut();
                if sf_mut
                    .fields
                    .push_work(self.work, self.core, (sf_mut.key, expr))
                    .is_none()
                {
                    return Err(self.oom(expr.span));
                }
                sf_mut.max_child = sf_mut.max_child.max(depth);
                if sf_mut.sexp {
                    self.expect(WorkTokenKind::RParen, "')'")?;
                    if self.peek().kind == WorkTokenKind::RParen {
                        self.advance()?;
                        let s = *sf.as_ref();
                        let se = self.alloc_box(
                            StructExpr {
                                name: s.name,
                                fields: s.fields,
                            },
                            s.span,
                        )?;
                        return Ok(NextStep::HaveExpr(
                            Expr::new(s.span, ExprKind::Struct(se)),
                            1 + s.max_child,
                        ));
                    }
                    if self.at_eof() {
                        return Err(self.parse_err(
                            "src.syntax",
                            sf.as_ref().brace_start,
                            "unterminated struct '('",
                        ));
                    }
                    self.expect(WorkTokenKind::LParen, "'('")?;
                    let (next_key, _) = self.expect_ident("field name")?;
                    sf.as_mut().key = next_key;
                    let sp = sf.as_ref().span;
                    self.push_frame(ParseFrame::StructField(sf), sp)?;
                    return Ok(NextStep::StartExpr);
                }
                if self.peek().kind == WorkTokenKind::RBrace {
                    self.advance()?;
                    let s = *sf.as_ref();
                    let se = self.alloc_box(
                        StructExpr {
                            name: s.name,
                            fields: s.fields,
                        },
                        s.span,
                    )?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(s.span, ExprKind::Struct(se)),
                        1 + s.max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(self.parse_err(
                        "src.syntax",
                        sf.as_ref().brace_start,
                        "unterminated '{'",
                    ));
                }
                let (next_key, _) = self.expect_ident("field name")?;
                self.expect(WorkTokenKind::Colon, "':'")?;
                sf.as_mut().key = next_key;
                let sp = sf.as_ref().span;
                self.push_frame(ParseFrame::StructField(sf), sp)?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::VariantData {
                span,
                type_name,
                variant,
            } => {
                self.expect(WorkTokenKind::RParen, "')'")?;
                let data = self.alloc_box(expr, span)?;
                let ve = self.alloc_box(
                    VariantExpr {
                        type_name,
                        variant,
                        data: Some(data),
                    },
                    span,
                )?;
                Ok(NextStep::HaveExpr(
                    Expr::new(span, ExprKind::Variant(ve)),
                    1 + depth,
                ))
            }
            ParseFrame::Def { start, name } => {
                self.expect(WorkTokenKind::RParen, "')'")?;
                let value = self.alloc_box(expr, start)?;
                Ok(NextStep::HaveExpr(
                    Expr::new(start, ExprKind::Def { name, value }),
                    1 + depth,
                ))
            }
            ParseFrame::Body(mut bf) => {
                let bf_mut = bf.as_mut();
                if bf_mut.body.push_work(self.work, self.core, expr).is_none() {
                    return Err(self.oom(expr.span));
                }
                bf_mut.max_child = bf_mut.max_child.max(depth);
                if self.peek().kind == WorkTokenKind::RParen {
                    self.advance()?;
                    let b = *bf.as_ref();
                    let finished = self.finish_body(b.start, b.body, b.target)?;
                    return Ok(NextStep::HaveExpr(finished, 1 + b.max_child));
                }
                if self.at_eof() {
                    return Err(self.parse_err(
                        "src.syntax",
                        bf.as_ref().start,
                        "unterminated '('",
                    ));
                }
                let sp = bf.as_ref().start;
                self.push_frame(ParseFrame::Body(bf), sp)?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::LetBindVal(mut lf) => {
                let lf_mut = lf.as_mut();
                if lf_mut
                    .bindings
                    .push_work(self.work, self.core, (lf_mut.key, expr))
                    .is_none()
                {
                    return Err(self.oom(expr.span));
                }
                lf_mut.max_child = lf_mut.max_child.max(depth);
                if self.peek().kind == WorkTokenKind::RParen {
                    self.advance()?;
                    let l = *lf.as_ref();
                    return self.start_nonempty_body(
                        l.start,
                        l.max_child,
                        BodyTarget::Let {
                            bindings: l.bindings,
                        },
                    );
                }
                if self.at_eof() {
                    return Err(self.parse_err(
                        "src.syntax",
                        lf.as_ref().bind_start,
                        "unterminated let bindings",
                    ));
                }
                let (next_key, _) = self.expect_ident("binding name")?;
                lf.as_mut().key = next_key;
                let sp = lf.as_ref().start;
                self.push_frame(ParseFrame::LetBindVal(lf), sp)?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::IfCond { start } => {
                let cond = self.alloc_box(expr, start)?;
                self.push_frame(
                    ParseFrame::IfThen {
                        start,
                        cond,
                        max_child: depth,
                    },
                    start,
                )?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::IfThen {
                start,
                cond,
                max_child,
            } => {
                if self.peek().kind == WorkTokenKind::RParen {
                    return Err(self.parse_err(
                        "expr.if",
                        start,
                        "'if' requires 3 arguments (cond, then, else); use 'when' when no value is needed",
                    ));
                }
                let then_branch = self.alloc_box(expr, start)?;
                self.push_frame(
                    ParseFrame::IfElse {
                        start,
                        cond,
                        then_branch,
                        max_child: max_child.max(depth),
                    },
                    start,
                )?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::IfElse {
                start,
                cond,
                then_branch,
                max_child,
            } => {
                self.expect(WorkTokenKind::RParen, "')'")?;
                let else_branch = self.alloc_box(expr, start)?;
                Ok(NextStep::HaveExpr(
                    Expr::new(
                        start,
                        ExprKind::If {
                            cond,
                            then_branch,
                            else_branch,
                        },
                    ),
                    1 + max_child.max(depth),
                ))
            }
            ParseFrame::WhenCond { start } => {
                let cond = self.alloc_box(expr, start)?;
                self.start_nonempty_body(start, depth, BodyTarget::When { cond })
            }
            ParseFrame::WithTarget { start } => {
                let target = self.alloc_box(expr, start)?;
                if self.peek().kind == WorkTokenKind::RParen {
                    self.advance()?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(
                            start,
                            ExprKind::With {
                                target,
                                fields: RawVec::new(),
                            },
                        ),
                        1 + depth,
                    ));
                }
                if self.peek().kind == WorkTokenKind::LParen {
                    self.advance()?;
                    let (key, _) = self.expect_ident("field name")?;
                    let wf = self.alloc_box(
                        WithFieldFrame {
                            start,
                            brace_start: start,
                            target,
                            fields: RawVec::new(),
                            key,
                            max_child: depth,
                            sexp: true,
                        },
                        start,
                    )?;
                    self.push_frame(ParseFrame::WithFieldVal(wf), start)?;
                    return Ok(NextStep::StartExpr);
                }
                let brace_start = self.expect(WorkTokenKind::LBrace, "'{' or '('")?;
                if self.peek().kind == WorkTokenKind::RBrace {
                    self.advance()?;
                    self.expect(WorkTokenKind::RParen, "')'")?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(
                            start,
                            ExprKind::With {
                                target,
                                fields: RawVec::new(),
                            },
                        ),
                        1 + depth,
                    ));
                }
                let (key, _) = self.expect_ident("field name")?;
                self.expect(WorkTokenKind::Colon, "':'")?;
                let wf = self.alloc_box(
                    WithFieldFrame {
                        start,
                        brace_start,
                        target,
                        fields: RawVec::new(),
                        key,
                        max_child: depth,
                        sexp: false,
                    },
                    start,
                )?;
                self.push_frame(ParseFrame::WithFieldVal(wf), start)?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::WithFieldVal(mut wf) => {
                let wf_mut = wf.as_mut();
                if wf_mut
                    .fields
                    .push_work(self.work, self.core, (wf_mut.key, expr))
                    .is_none()
                {
                    return Err(self.oom(expr.span));
                }
                wf_mut.max_child = wf_mut.max_child.max(depth);
                if wf_mut.sexp {
                    self.expect(WorkTokenKind::RParen, "')'")?;
                    if self.peek().kind == WorkTokenKind::RParen {
                        self.advance()?;
                        let w = *wf.as_ref();
                        return Ok(NextStep::HaveExpr(
                            Expr::new(
                                w.start,
                                ExprKind::With {
                                    target: w.target,
                                    fields: w.fields,
                                },
                            ),
                            1 + w.max_child,
                        ));
                    }
                    if self.at_eof() {
                        return Err(self.parse_err(
                            "src.syntax",
                            wf.as_ref().start,
                            "unterminated with '('",
                        ));
                    }
                    self.expect(WorkTokenKind::LParen, "'('")?;
                    let (next_key, _) = self.expect_ident("field name")?;
                    wf.as_mut().key = next_key;
                    let sp = wf.as_ref().start;
                    self.push_frame(ParseFrame::WithFieldVal(wf), sp)?;
                    return Ok(NextStep::StartExpr);
                }
                if self.peek().kind == WorkTokenKind::RBrace {
                    self.advance()?;
                    self.expect(WorkTokenKind::RParen, "')'")?;
                    let w = *wf.as_ref();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(
                            w.start,
                            ExprKind::With {
                                target: w.target,
                                fields: w.fields,
                            },
                        ),
                        1 + w.max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(self.parse_err(
                        "src.syntax",
                        wf.as_ref().brace_start,
                        "unterminated '{'",
                    ));
                }
                let (next_key, _) = self.expect_ident("field name")?;
                self.expect(WorkTokenKind::Colon, "':'")?;
                wf.as_mut().key = next_key;
                let sp = wf.as_ref().start;
                self.push_frame(ParseFrame::WithFieldVal(wf), sp)?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::MatchScrutinee { start } => {
                let scrutinee = self.alloc_box(expr, start)?;
                self.start_next_match_arm(start, scrutinee, RawVec::new(), depth)
            }
            ParseFrame::MatchArmBody(mut mf) => {
                let mf_mut = mf.as_mut();
                if mf_mut.body.push_work(self.work, self.core, expr).is_none() {
                    return Err(self.oom(expr.span));
                }
                mf_mut.max_child = mf_mut.max_child.max(depth);
                if self.peek().kind == WorkTokenKind::RParen {
                    self.advance()?;
                    let arm = MatchArm {
                        span: mf.as_ref().arm_start,
                        pattern: mf.as_ref().pattern,
                        body: mf.as_ref().body,
                    };
                    if mf
                        .as_mut()
                        .arms
                        .push_work(self.work, self.core, arm)
                        .is_none()
                    {
                        return Err(self.oom(arm.span));
                    }
                    let m = *mf.as_ref();
                    return self.start_next_match_arm(m.start, m.scrutinee, m.arms, m.max_child);
                }
                if self.at_eof() {
                    return Err(self.parse_err(
                        "src.syntax",
                        mf.as_ref().arm_start,
                        "unterminated '('",
                    ));
                }
                let sp = mf.as_ref().arm_start;
                self.push_frame(ParseFrame::MatchArmBody(mf), sp)?;
                Ok(NextStep::StartExpr)
            }
            ParseFrame::DatumUnquote {
                span,
                is_paren,
                is_splicing,
            } => {
                if is_paren {
                    self.expect(WorkTokenKind::RParen, "')'")?;
                }
                let boxed = self.alloc_box(expr, span)?;
                let kind = if is_splicing {
                    DatumKind::UnquoteSplicing(boxed)
                } else {
                    DatumKind::Unquote(boxed)
                };
                Ok(NextStep::HaveDatum(
                    Datum::new(span, kind),
                    1 + depth,
                    QqState::QUASIQUOTE,
                ))
            }
            ParseFrame::QuoteFinish { .. }
            | ParseFrame::DatumWrap { .. }
            | ParseFrame::DatumList(_)
            | ParseFrame::DatumIndex { .. }
            | ParseFrame::DatumVariantData { .. }
            | ParseFrame::DatumStructFields(_) => {
                unreachable!("unexpected datum frame in resume_frame")
            }
        }
    }
}
