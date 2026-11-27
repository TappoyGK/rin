use super::*;

impl Parser<'_, '_, '_> {
    pub(super) fn start_primary(&mut self) -> Result<NextStep, ErrRecorded> {
        let tok = self.peek();
        let span = tok.span;
        match tok.kind {
            WorkTokenKind::Int(n) => {
                self.advance()?;
                Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::Int(n)), 1))
            }
            WorkTokenKind::Float(x) => {
                self.advance()?;
                Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::Float(x)), 1))
            }
            WorkTokenKind::Str(s) => {
                self.advance()?;
                Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::Str(s)), 1))
            }
            WorkTokenKind::TemplateStart(s) => {
                self.advance()?;
                let mut parts = RawVec::new();
                if !s.is_empty()
                    && parts
                        .push_work(self.work, self.core, Expr::new(span, ExprKind::Str(s)))
                        .is_none()
                {
                    return Err(self.oom(span));
                }
                self.push_frame(
                    ParseFrame::Template {
                        start: span,
                        parts,
                        max_child: 0,
                    },
                    span,
                )?;
                Ok(NextStep::StartExpr)
            }
            WorkTokenKind::Char(c) => {
                self.advance()?;
                Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::Char(c)), 1))
            }
            WorkTokenKind::Quote => {
                self.advance()?;
                self.push_frame(
                    ParseFrame::QuoteFinish {
                        span,
                        is_paren: false,
                        is_quasiquote: false,
                    },
                    span,
                )?;
                Ok(NextStep::StartDatum {
                    qq: QqState::QUOTE,
                    allow_splice: false,
                })
            }
            WorkTokenKind::Quasiquote => {
                self.advance()?;
                self.push_frame(
                    ParseFrame::QuoteFinish {
                        span,
                        is_paren: false,
                        is_quasiquote: true,
                    },
                    span,
                )?;
                Ok(NextStep::StartDatum {
                    qq: QqState::QUASIQUOTE,
                    allow_splice: false,
                })
            }
            WorkTokenKind::Unquote => Err(self.parse_err(
                "src.syntax",
                span,
                "unquote ',' is only allowed inside quasiquote",
            )),
            WorkTokenKind::UnquoteSplicing => Err(self.parse_err(
                "src.syntax",
                span,
                "unquote-splicing ',@' is only allowed inside quasiquote",
            )),
            WorkTokenKind::LBracket => {
                self.advance()?;
                if self.peek().kind == WorkTokenKind::RBracket {
                    self.advance()?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(span, ExprKind::List(RawVec::new())),
                        1,
                    ));
                }
                self.push_frame(
                    ParseFrame::List {
                        start: span,
                        items: RawVec::new(),
                        max_child: 0,
                    },
                    span,
                )?;
                Ok(NextStep::StartExpr)
            }
            WorkTokenKind::LParen => self.start_paren(),
            WorkTokenKind::Ident(name) => self.start_ident(name, span),
            other => Err(self.parse_err_fmt(
                "src.syntax",
                span,
                format_args!("unexpected token {other:?}"),
            )),
        }
    }

    fn start_ident(&mut self, name: RawStr, span: Span) -> Result<NextStep, ErrRecorded> {
        self.advance()?;
        match name.as_str() {
            "true" => return Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::Bool(true)), 1)),
            "false" => {
                return Ok(NextStep::HaveExpr(
                    Expr::new(span, ExprKind::Bool(false)),
                    1,
                ));
            }
            "none" => return Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::None), 1)),
            "inf" => {
                return Ok(NextStep::HaveExpr(
                    Expr::new(span, ExprKind::Float(f64::INFINITY)),
                    1,
                ));
            }
            "nan" => {
                return Ok(NextStep::HaveExpr(
                    Expr::new(span, ExprKind::Float(f64::NAN)),
                    1,
                ));
            }
            "some" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                self.advance()?;
                self.push_frame(
                    ParseFrame::BuiltinWrap {
                        span,
                        kind: WrapKind::Some,
                    },
                    span,
                )?;
                return Ok(NextStep::StartExpr);
            }
            "ok" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                self.advance()?;
                self.push_frame(
                    ParseFrame::BuiltinWrap {
                        span,
                        kind: WrapKind::Ok,
                    },
                    span,
                )?;
                return Ok(NextStep::StartExpr);
            }
            "err" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                self.advance()?;
                self.push_frame(
                    ParseFrame::BuiltinWrap {
                        span,
                        kind: WrapKind::Err,
                    },
                    span,
                )?;
                return Ok(NextStep::StartExpr);
            }
            _ => {}
        }

        let is_upper = name
            .as_str()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase());
        if is_upper && self.peek().kind == WorkTokenKind::LBrace {
            let brace_start = self.peek().span;
            self.advance()?;
            if self.peek().kind == WorkTokenKind::RBrace {
                self.advance()?;
                let s = self.alloc_box(
                    StructExpr {
                        name,
                        fields: RawVec::new(),
                    },
                    span,
                )?;
                return Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::Struct(s)), 1));
            }
            let (key, _) = self.expect_ident("field name")?;
            self.expect(WorkTokenKind::Colon, "':'")?;
            let sf = self.alloc_box(
                StructFieldFrame {
                    brace_start,
                    span,
                    name,
                    fields: RawVec::new(),
                    key,
                    max_child: 0,
                    sexp: false,
                },
                span,
            )?;
            self.push_frame(ParseFrame::StructField(sf), span)?;
            return Ok(NextStep::StartExpr);
        }

        if is_upper && self.peek().kind == WorkTokenKind::Dot {
            self.advance()?;
            let (variant, _) = self.expect_ident("variant name after '.'")?;
            if self.peek().kind == WorkTokenKind::LParen && self.peek().attached {
                self.advance()?;
                self.push_frame(
                    ParseFrame::VariantData {
                        span,
                        type_name: name,
                        variant,
                    },
                    span,
                )?;
                return Ok(NextStep::StartExpr);
            }
            let v = self.alloc_box(
                VariantExpr {
                    type_name: name,
                    variant,
                    data: None,
                },
                span,
            )?;
            return Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::Variant(v)), 1));
        }

        Ok(NextStep::HaveExpr(
            Expr::new(span, ExprKind::Ident(name)),
            1,
        ))
    }

    fn start_paren(&mut self) -> Result<NextStep, ErrRecorded> {
        let start = self.expect(WorkTokenKind::LParen, "'('")?;
        if self.peek().kind == WorkTokenKind::RParen {
            return Err(self.parse_err(
                "src.syntax",
                start,
                "empty '()' is not a valid expression",
            ));
        }

        // 先頭が '.' や '[' に続かない識別子の場合は特殊形式を確認する。
        let special = if let WorkTokenKind::Ident(head) = self.peek().kind {
            let next_tok = self.peek_next()?;
            let is_plain_head = !matches!(next_tok.map(|t| t.kind), Some(WorkTokenKind::Dot))
                && !matches!(
                    next_tok,
                    Some(WorkToken {
                        kind: WorkTokenKind::LBracket,
                        attached: true,
                        ..
                    })
                );
            if is_plain_head { Some(head) } else { None }
        } else {
            None
        };

        if let Some(head) = special {
            if !self.test_mode && matches!(head.as_str(), "deftest" | "check" | "check_eq") {
                return Err(if head.as_str() == "deftest" {
                    self.parse_err(
                        "test.deftest",
                        start,
                        "deftest is only allowed in test files",
                    )
                } else {
                    self.parse_err(
                        "test.check",
                        start,
                        "checks are only allowed inside deftest",
                    )
                });
            }
            match head.as_str() {
                "def" => {
                    self.advance()?; // def
                    let (name, _) = self.expect_ident("variable name")?;
                    self.push_frame(ParseFrame::Def { start, name }, start)?;
                    return Ok(NextStep::StartExpr);
                }
                "import" => {
                    self.advance()?; // import
                    let (name, _) = self.expect_ident("module name")?;
                    self.expect(WorkTokenKind::RParen, "')'")?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::Import { name }),
                        1,
                    ));
                }
                "export" => {
                    self.advance()?; // export
                    let mut names = RawVec::new();
                    while self.peek().kind != WorkTokenKind::RParen {
                        if self.at_eof() {
                            return Err(self.parse_err(
                                "src.syntax",
                                start,
                                "unterminated export '('",
                            ));
                        }
                        let (name, nspan) = self.expect_ident("exported symbol name")?;
                        if names.push_work(self.work, self.core, name).is_none() {
                            return Err(self.oom(nspan));
                        }
                    }
                    self.advance()?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::Export { names }),
                        1,
                    ));
                }
                "defn" => {
                    self.advance()?; // defn
                    let (name, _) = self.expect_ident("function name")?;
                    let params = self.parse_param_list()?;
                    return self.start_nonempty_body(start, 0, BodyTarget::Defn { name, params });
                }
                "fn" => {
                    self.advance()?; // fn
                    let params = self.parse_param_list()?;
                    return self.start_nonempty_body(start, 0, BodyTarget::Fn { params });
                }
                "let" => {
                    self.advance()?; // let
                    let bind_start = self.expect(WorkTokenKind::LParen, "let bindings '('")?;
                    if self.peek().kind == WorkTokenKind::RParen {
                        self.advance()?;
                        return self.start_nonempty_body(
                            start,
                            0,
                            BodyTarget::Let {
                                bindings: RawVec::new(),
                            },
                        );
                    }
                    let (key, _) = self.expect_ident("binding name")?;
                    let lf = self.alloc_box(
                        LetBindFrame {
                            start,
                            bind_start,
                            bindings: RawVec::new(),
                            key,
                            max_child: 0,
                        },
                        start,
                    )?;
                    self.push_frame(ParseFrame::LetBindVal(lf), start)?;
                    return Ok(NextStep::StartExpr);
                }
                "if" => {
                    self.advance()?; // if
                    self.push_frame(ParseFrame::IfCond { start }, start)?;
                    return Ok(NextStep::StartExpr);
                }
                "when" => {
                    self.advance()?; // when
                    self.push_frame(ParseFrame::WhenCond { start }, start)?;
                    return Ok(NextStep::StartExpr);
                }
                "with" => {
                    self.advance()?; // with
                    self.push_frame(ParseFrame::WithTarget { start }, start)?;
                    return Ok(NextStep::StartExpr);
                }
                "match" => {
                    self.advance()?; // match
                    self.push_frame(ParseFrame::MatchScrutinee { start }, start)?;
                    return Ok(NextStep::StartExpr);
                }
                "quote" => {
                    self.advance()?; // quote
                    self.push_frame(
                        ParseFrame::QuoteFinish {
                            span: start,
                            is_paren: true,
                            is_quasiquote: false,
                        },
                        start,
                    )?;
                    return Ok(NextStep::StartDatum {
                        qq: QqState::QUOTE,
                        allow_splice: false,
                    });
                }
                "quasiquote" => {
                    self.advance()?; // quasiquote
                    self.push_frame(
                        ParseFrame::QuoteFinish {
                            span: start,
                            is_paren: true,
                            is_quasiquote: true,
                        },
                        start,
                    )?;
                    return Ok(NextStep::StartDatum {
                        qq: QqState::QUASIQUOTE,
                        allow_splice: false,
                    });
                }
                "unquote" => {
                    return Err(self.parse_err(
                        "src.syntax",
                        start,
                        "'unquote' is only allowed inside quasiquote",
                    ));
                }
                "unquote_splicing" => {
                    return Err(self.parse_err(
                        "src.syntax",
                        start,
                        "'unquote_splicing' is only allowed inside quasiquote",
                    ));
                }
                "tuple" => {
                    self.advance()?; // tuple
                    if self.peek().kind == WorkTokenKind::RParen {
                        return Err(self.parse_err(
                            "src.syntax",
                            start,
                            "tuple requires at least 2 elements",
                        ));
                    }
                    self.push_frame(
                        ParseFrame::Tuple {
                            start,
                            items: RawVec::new(),
                            max_child: 0,
                        },
                        start,
                    )?;
                    return Ok(NextStep::StartExpr);
                }
                "field" => {
                    self.advance()?; // field
                    self.push_frame(ParseFrame::FieldTarget { start }, start)?;
                    return Ok(NextStep::StartExpr);
                }
                "index" => {
                    self.advance()?; // index
                    self.push_frame(ParseFrame::IndexTarget { start }, start)?;
                    return Ok(NextStep::StartExpr);
                }
                "some" => {
                    self.advance()?; // some
                    self.push_frame(
                        ParseFrame::BuiltinWrap {
                            span: start,
                            kind: WrapKind::Some,
                        },
                        start,
                    )?;
                    return Ok(NextStep::StartExpr);
                }
                "ok" => {
                    self.advance()?; // ok
                    self.push_frame(
                        ParseFrame::BuiltinWrap {
                            span: start,
                            kind: WrapKind::Ok,
                        },
                        start,
                    )?;
                    return Ok(NextStep::StartExpr);
                }
                "err" => {
                    self.advance()?; // err
                    self.push_frame(
                        ParseFrame::BuiltinWrap {
                            span: start,
                            kind: WrapKind::Err,
                        },
                        start,
                    )?;
                    return Ok(NextStep::StartExpr);
                }
                "variant" => {
                    self.advance()?; // variant
                    let (type_name, _) = self.expect_ident("type name")?;
                    let (variant, _) = self.expect_ident("variant name")?;
                    if self.peek().kind == WorkTokenKind::RParen {
                        self.advance()?;
                        let v = self.alloc_box(
                            VariantExpr {
                                type_name,
                                variant,
                                data: None,
                            },
                            start,
                        )?;
                        return Ok(NextStep::HaveExpr(
                            Expr::new(start, ExprKind::Variant(v)),
                            1,
                        ));
                    }
                    self.push_frame(
                        ParseFrame::VariantData {
                            span: start,
                            type_name,
                            variant,
                        },
                        start,
                    )?;
                    return Ok(NextStep::StartExpr);
                }
                "struct" => {
                    self.advance()?; // struct
                    let (name, _) = self.expect_ident("struct name")?;
                    if self.peek().kind == WorkTokenKind::RParen {
                        self.advance()?;
                        let s = self.alloc_box(
                            StructExpr {
                                name,
                                fields: RawVec::new(),
                            },
                            start,
                        )?;
                        return Ok(NextStep::HaveExpr(Expr::new(start, ExprKind::Struct(s)), 1));
                    }
                    self.expect(WorkTokenKind::LParen, "'('")?;
                    let (key, _) = self.expect_ident("field name")?;
                    let sf = self.alloc_box(
                        StructFieldFrame {
                            brace_start: start,
                            span: start,
                            name,
                            fields: RawVec::new(),
                            key,
                            max_child: 0,
                            sexp: true,
                        },
                        start,
                    )?;
                    self.push_frame(ParseFrame::StructField(sf), start)?;
                    return Ok(NextStep::StartExpr);
                }
                _ => {}
            }
        }

        self.push_frame(ParseFrame::ParenFirst { start }, start)?;
        Ok(NextStep::StartExpr)
    }

    pub(super) fn start_nonempty_body(
        &mut self,
        start: Span,
        max_child: usize,
        target: BodyTarget,
    ) -> Result<NextStep, ErrRecorded> {
        if self.peek().kind == WorkTokenKind::RParen {
            return Err(self.parse_err("expr.body", start, "body cannot be empty"));
        }
        let bf = self.alloc_box(
            BodyFrame {
                start,
                body: RawVec::new(),
                max_child,
                target,
            },
            start,
        )?;
        self.push_frame(ParseFrame::Body(bf), start)?;
        Ok(NextStep::StartExpr)
    }

    pub(super) fn finish_body(
        &mut self,
        start: Span,
        body: RawVec<Expr>,
        target: BodyTarget,
    ) -> Result<Expr, ErrRecorded> {
        let kind = match target {
            BodyTarget::Defn { name, params } => {
                ExprKind::Defn(self.alloc_box(DefnExpr { name, params, body }, start)?)
            }
            BodyTarget::Fn { params } => {
                ExprKind::Fn(self.alloc_box(FnExpr { params, body }, start)?)
            }
            BodyTarget::Let { bindings } => {
                ExprKind::Let(self.alloc_box(LetExpr { bindings, body }, start)?)
            }
            BodyTarget::When { cond } => ExprKind::When { cond, body },
        };
        Ok(Expr::new(start, kind))
    }

    pub(super) fn start_next_match_arm(
        &mut self,
        start: Span,
        scrutinee: WorkBox<Expr>,
        arms: RawVec<MatchArm>,
        max_child: usize,
    ) -> Result<NextStep, ErrRecorded> {
        if self.peek().kind == WorkTokenKind::RParen {
            self.advance()?;
            return Ok(NextStep::HaveExpr(
                Expr::new(start, ExprKind::Match { scrutinee, arms }),
                1 + max_child,
            ));
        }
        if self.at_eof() {
            return Err(self.parse_err("src.syntax", start, "unterminated match"));
        }
        let arm_start = self.expect(WorkTokenKind::LParen, "match arm '('")?;
        let pattern = self.parse_pattern()?;
        if self.peek().kind == WorkTokenKind::RParen {
            return Err(self.parse_err("expr.body", arm_start, "body cannot be empty"));
        }
        let mf = self.alloc_box(
            MatchArmFrame {
                start,
                scrutinee,
                arms,
                arm_start,
                pattern,
                body: RawVec::new(),
                max_child,
            },
            arm_start,
        )?;
        self.push_frame(ParseFrame::MatchArmBody(mf), arm_start)?;
        Ok(NextStep::StartExpr)
    }
}
