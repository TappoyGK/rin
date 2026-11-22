use super::*;

impl Parser<'_> {
    pub(super) fn resume_frame(
        &mut self,
        frame: ParseFrame,
        expr: Expr,
        depth: usize,
    ) -> Result<NextStep, Error> {
        match frame {
            ParseFrame::PostfixIndex {
                target,
                target_depth,
            } => {
                self.expect(TokenKind::RBracket, "']'")?;
                let combined = 1 + target_depth.max(depth);
                Self::check_nesting(self.frames.len() + combined, target.span)?;
                let span = target.span;
                Ok(NextStep::HaveExpr(
                    Expr::new(
                        span,
                        ExprKind::Index {
                            target,
                            index: Box::new(expr),
                        },
                    ),
                    combined,
                ))
            }
            ParseFrame::List {
                start,
                mut items,
                mut max_child,
            } => {
                items.push(expr);
                max_child = max_child.max(depth);
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                }
                if self.peek().kind == TokenKind::RBracket {
                    self.advance();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::List(items)),
                        1 + max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(Error::parse("src.syntax", start, "unterminated '['"));
                }
                self.frames.push(ParseFrame::List {
                    start,
                    items,
                    max_child,
                });
                Ok(NextStep::StartExpr)
            }
            ParseFrame::ParenFirst { start } => {
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                    let items = vec![expr];
                    if self.peek().kind == TokenKind::RParen {
                        self.advance();
                        return Ok(NextStep::HaveExpr(
                            Expr::new(start, ExprKind::Tuple(items)),
                            1 + depth,
                        ));
                    }
                    if self.at_eof() {
                        return Err(Error::parse("src.syntax", start, "unterminated tuple '('"));
                    }
                    self.frames.push(ParseFrame::Tuple {
                        start,
                        items,
                        max_child: depth,
                    });
                    return Ok(NextStep::StartExpr);
                }
                if self.peek().kind == TokenKind::RParen {
                    self.advance();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(
                            start,
                            ExprKind::Call {
                                callee: Box::new(expr),
                                args: Vec::new(),
                            },
                        ),
                        1 + depth,
                    ));
                }
                if self.at_eof() {
                    return Err(Error::parse("src.syntax", start, "unterminated '('"));
                }
                self.frames.push(ParseFrame::Call {
                    start,
                    callee: Box::new(expr),
                    args: Vec::new(),
                    max_child: depth,
                });
                Ok(NextStep::StartExpr)
            }
            ParseFrame::Tuple {
                start,
                mut items,
                mut max_child,
            } => {
                items.push(expr);
                max_child = max_child.max(depth);
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                    if self.peek().kind == TokenKind::RParen {
                        self.advance();
                        return Ok(NextStep::HaveExpr(
                            Expr::new(start, ExprKind::Tuple(items)),
                            1 + max_child,
                        ));
                    }
                    if self.at_eof() {
                        return Err(Error::parse("src.syntax", start, "unterminated tuple '('"));
                    }
                    self.frames.push(ParseFrame::Tuple {
                        start,
                        items,
                        max_child,
                    });
                    return Ok(NextStep::StartExpr);
                }
                self.expect(TokenKind::RParen, "')'")?;
                Ok(NextStep::HaveExpr(
                    Expr::new(start, ExprKind::Tuple(items)),
                    1 + max_child,
                ))
            }
            ParseFrame::Call {
                start,
                callee,
                mut args,
                mut max_child,
            } => {
                args.push(expr);
                max_child = max_child.max(depth);
                if self.peek().kind == TokenKind::RParen {
                    self.advance();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::Call { callee, args }),
                        1 + max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(Error::parse("src.syntax", start, "unterminated '('"));
                }
                self.frames.push(ParseFrame::Call {
                    start,
                    callee,
                    args,
                    max_child,
                });
                Ok(NextStep::StartExpr)
            }
            ParseFrame::BuiltinWrap { span, kind } => {
                self.expect(TokenKind::RParen, "')'")?;
                let boxed = Box::new(expr);
                let ek = match kind {
                    WrapKind::Some => ExprKind::Some(boxed),
                    WrapKind::Ok => ExprKind::Ok(boxed),
                    WrapKind::Err => ExprKind::Err(boxed),
                };
                Ok(NextStep::HaveExpr(Expr::new(span, ek), 1 + depth))
            }
            ParseFrame::StructField(mut sf) => {
                sf.fields.push((sf.key, expr));
                sf.max_child = sf.max_child.max(depth);
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                }
                if self.peek().kind == TokenKind::RBrace {
                    self.advance();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(
                            sf.span,
                            ExprKind::Struct {
                                name: sf.name,
                                fields: sf.fields,
                            },
                        ),
                        1 + sf.max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(Error::parse(
                        "src.syntax",
                        sf.brace_start,
                        "unterminated '{'",
                    ));
                }
                let (next_key, _) = self.expect_ident("field name")?;
                self.expect(TokenKind::Colon, "':'")?;
                sf.key = next_key;
                self.frames.push(ParseFrame::StructField(sf));
                Ok(NextStep::StartExpr)
            }
            ParseFrame::VariantData {
                span,
                type_name,
                variant,
            } => {
                self.expect(TokenKind::RParen, "')'")?;
                Ok(NextStep::HaveExpr(
                    Expr::new(
                        span,
                        ExprKind::Variant {
                            type_name,
                            variant,
                            data: Some(Box::new(expr)),
                        },
                    ),
                    1 + depth,
                ))
            }
            ParseFrame::Def { start, name } => {
                self.expect(TokenKind::RParen, "')'")?;
                Ok(NextStep::HaveExpr(
                    Expr::new(
                        start,
                        ExprKind::Def {
                            name,
                            value: Box::new(expr),
                        },
                    ),
                    1 + depth,
                ))
            }
            ParseFrame::Body(mut bf) => {
                bf.body.push(expr);
                bf.max_child = bf.max_child.max(depth);
                if self.peek().kind == TokenKind::RParen {
                    self.advance();
                    let mc = bf.max_child;
                    return Ok(NextStep::HaveExpr(
                        Self::finish_body(bf.start, bf.body, bf.target),
                        1 + mc,
                    ));
                }
                if self.at_eof() {
                    return Err(Error::parse("src.syntax", bf.start, "unterminated '('"));
                }
                self.frames.push(ParseFrame::Body(bf));
                Ok(NextStep::StartExpr)
            }
            ParseFrame::LetBindVal(mut lf) => {
                lf.bindings.push((lf.key, expr));
                lf.max_child = lf.max_child.max(depth);
                if self.peek().kind == TokenKind::RParen {
                    self.advance();
                    return self.start_nonempty_body(
                        lf.start,
                        lf.max_child,
                        BodyTarget::Let {
                            bindings: lf.bindings,
                        },
                    );
                }
                if self.at_eof() {
                    return Err(Error::parse(
                        "src.syntax",
                        lf.bind_start,
                        "unterminated let bindings",
                    ));
                }
                let (next_key, _) = self.expect_ident("binding name")?;
                lf.key = next_key;
                self.frames.push(ParseFrame::LetBindVal(lf));
                Ok(NextStep::StartExpr)
            }
            ParseFrame::IfCond { start } => {
                self.frames.push(ParseFrame::IfThen {
                    start,
                    cond: Box::new(expr),
                    max_child: depth,
                });
                Ok(NextStep::StartExpr)
            }
            ParseFrame::IfThen {
                start,
                cond,
                max_child,
            } => {
                if self.peek().kind == TokenKind::RParen {
                    return Err(Error::parse(
                        "expr.if",
                        start,
                        "'if' requires 3 arguments (cond, then, else); use 'when' when no value is needed",
                    ));
                }
                self.frames.push(ParseFrame::IfElse {
                    start,
                    cond,
                    then_branch: Box::new(expr),
                    max_child: max_child.max(depth),
                });
                Ok(NextStep::StartExpr)
            }
            ParseFrame::IfElse {
                start,
                cond,
                then_branch,
                max_child,
            } => {
                self.expect(TokenKind::RParen, "')'")?;
                Ok(NextStep::HaveExpr(
                    Expr::new(
                        start,
                        ExprKind::If {
                            cond,
                            then_branch,
                            else_branch: Box::new(expr),
                        },
                    ),
                    1 + max_child.max(depth),
                ))
            }
            ParseFrame::WhenCond { start } => self.start_nonempty_body(
                start,
                depth,
                BodyTarget::When {
                    cond: Box::new(expr),
                },
            ),
            ParseFrame::WithTarget { start } => {
                let brace_start = self.expect(TokenKind::LBrace, "'{'")?;
                let target = Box::new(expr);
                if self.peek().kind == TokenKind::RBrace {
                    self.advance();
                    self.expect(TokenKind::RParen, "')'")?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(
                            start,
                            ExprKind::With {
                                target,
                                fields: Vec::new(),
                            },
                        ),
                        1 + depth,
                    ));
                }
                let (key, _) = self.expect_ident("field name")?;
                self.expect(TokenKind::Colon, "':'")?;
                self.frames
                    .push(ParseFrame::WithFieldVal(Box::new(WithFieldFrame {
                        start,
                        brace_start,
                        target,
                        fields: Vec::new(),
                        key,
                        max_child: depth,
                    })));
                Ok(NextStep::StartExpr)
            }
            ParseFrame::WithFieldVal(mut wf) => {
                wf.fields.push((wf.key, expr));
                wf.max_child = wf.max_child.max(depth);
                if self.peek().kind == TokenKind::Comma {
                    self.advance();
                }
                if self.peek().kind == TokenKind::RBrace {
                    self.advance();
                    self.expect(TokenKind::RParen, "')'")?;
                    let mc = wf.max_child;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(
                            wf.start,
                            ExprKind::With {
                                target: wf.target,
                                fields: wf.fields,
                            },
                        ),
                        1 + mc,
                    ));
                }
                if self.at_eof() {
                    return Err(Error::parse(
                        "src.syntax",
                        wf.brace_start,
                        "unterminated '{'",
                    ));
                }
                let (next_key, _) = self.expect_ident("field name")?;
                self.expect(TokenKind::Colon, "':'")?;
                wf.key = next_key;
                self.frames.push(ParseFrame::WithFieldVal(wf));
                Ok(NextStep::StartExpr)
            }
            ParseFrame::MatchScrutinee { start } => {
                self.start_next_match_arm(start, Box::new(expr), Vec::new(), depth)
            }
            ParseFrame::MatchArmBody(mut mf) => {
                mf.body.push(expr);
                mf.max_child = mf.max_child.max(depth);
                if self.peek().kind == TokenKind::RParen {
                    self.advance();
                    mf.arms.push(MatchArm {
                        span: mf.arm_start,
                        pattern: mf.pattern,
                        body: mf.body,
                    });
                    return self.start_next_match_arm(
                        mf.start,
                        mf.scrutinee,
                        mf.arms,
                        mf.max_child,
                    );
                }
                if self.at_eof() {
                    return Err(Error::parse("src.syntax", mf.arm_start, "unterminated '('"));
                }
                self.frames.push(ParseFrame::MatchArmBody(mf));
                Ok(NextStep::StartExpr)
            }
        }
    }
}
