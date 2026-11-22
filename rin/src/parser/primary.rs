use super::*;

impl Parser<'_> {
    pub(super) fn start_primary(&mut self) -> Result<NextStep, Error> {
        let span = self.peek().span;
        match &self.peek().kind {
            TokenKind::Int(n) => {
                let n = *n;
                self.advance();
                Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::Int(n)), 1))
            }
            TokenKind::Float(x) => {
                let x = *x;
                self.advance();
                Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::Float(x)), 1))
            }
            TokenKind::Str(s) => {
                let s = s.clone();
                self.advance();
                Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::Str(s)), 1))
            }
            TokenKind::Char(c) => {
                let c = *c;
                self.advance();
                Ok(NextStep::HaveExpr(Expr::new(span, ExprKind::Char(c)), 1))
            }
            TokenKind::LBracket => {
                self.advance();
                if self.peek().kind == TokenKind::RBracket {
                    self.advance();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(span, ExprKind::List(Vec::new())),
                        1,
                    ));
                }
                self.frames.push(ParseFrame::List {
                    start: span,
                    items: Vec::new(),
                    max_child: 0,
                });
                Ok(NextStep::StartExpr)
            }
            TokenKind::LParen => self.start_paren(),
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.start_ident(name, span)
            }
            other => Err(Error::parse(
                "src.syntax",
                span,
                format!("unexpected token {other:?}"),
            )),
        }
    }

    fn start_ident(&mut self, name: String, span: Span) -> Result<NextStep, Error> {
        self.advance();
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
            "some" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                self.advance();
                self.frames.push(ParseFrame::BuiltinWrap {
                    span,
                    kind: WrapKind::Some,
                });
                return Ok(NextStep::StartExpr);
            }
            "ok" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                self.advance();
                self.frames.push(ParseFrame::BuiltinWrap {
                    span,
                    kind: WrapKind::Ok,
                });
                return Ok(NextStep::StartExpr);
            }
            "err" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                self.advance();
                self.frames.push(ParseFrame::BuiltinWrap {
                    span,
                    kind: WrapKind::Err,
                });
                return Ok(NextStep::StartExpr);
            }
            _ => {}
        }

        let is_upper = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        if is_upper && self.peek().kind == TokenKind::LBrace {
            let brace_start = self.peek().span;
            self.advance();
            if self.peek().kind == TokenKind::RBrace {
                self.advance();
                return Ok(NextStep::HaveExpr(
                    Expr::new(
                        span,
                        ExprKind::Struct {
                            name,
                            fields: Vec::new(),
                        },
                    ),
                    1,
                ));
            }
            let (key, _) = self.expect_ident("field name")?;
            self.expect(TokenKind::Colon, "':'")?;
            self.frames
                .push(ParseFrame::StructField(Box::new(StructFieldFrame {
                    brace_start,
                    span,
                    name,
                    fields: Vec::new(),
                    key,
                    max_child: 0,
                })));
            return Ok(NextStep::StartExpr);
        }

        if is_upper && self.peek().kind == TokenKind::Dot {
            self.advance();
            let (variant, _) = self.expect_ident("variant name after '.'")?;
            if self.peek().kind == TokenKind::LParen && self.peek().attached {
                self.advance();
                self.frames.push(ParseFrame::VariantData {
                    span,
                    type_name: name,
                    variant,
                });
                return Ok(NextStep::StartExpr);
            }
            return Ok(NextStep::HaveExpr(
                Expr::new(
                    span,
                    ExprKind::Variant {
                        type_name: name,
                        variant,
                        data: None,
                    },
                ),
                1,
            ));
        }

        Ok(NextStep::HaveExpr(
            Expr::new(span, ExprKind::Ident(name)),
            1,
        ))
    }

    fn start_paren(&mut self) -> Result<NextStep, Error> {
        let start = self.expect(TokenKind::LParen, "'('")?;
        if self.peek().kind == TokenKind::RParen {
            return Err(Error::parse(
                "src.syntax",
                start,
                "empty '()' is not a valid expression",
            ));
        }

        // 先頭が '.' や '[' に続かない識別子の場合は特殊形式を確認する。
        let special = if let TokenKind::Ident(head) = &self.peek().kind {
            let is_plain_head = !matches!(
                self.peek_next().map(|t| &t.kind),
                Some(TokenKind::Dot) | Some(TokenKind::Comma)
            ) && !matches!(
                self.peek_next(),
                Some(Token {
                    kind: TokenKind::LBracket,
                    attached: true,
                    ..
                })
            );
            if is_plain_head {
                Some(head.clone())
            } else {
                None
            }
        } else {
            None
        };

        if let Some(head) = special {
            if !self.test_mode && matches!(head.as_str(), "deftest" | "check" | "check_eq") {
                return Err(if head == "deftest" {
                    Error::parse(
                        "test.deftest",
                        start,
                        "deftest is only allowed in test files",
                    )
                } else {
                    Error::parse(
                        "test.check",
                        start,
                        "checks are only allowed inside deftest",
                    )
                });
            }
            match head.as_str() {
                "def" => {
                    self.advance(); // def
                    let (name, _) = self.expect_ident("variable name")?;
                    self.frames.push(ParseFrame::Def { start, name });
                    return Ok(NextStep::StartExpr);
                }
                "import" => {
                    self.advance(); // import
                    let (name, _) = self.expect_ident("module name")?;
                    self.expect(TokenKind::RParen, "')'")?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::Import { name }),
                        1,
                    ));
                }
                "export" => {
                    self.advance(); // export
                    let mut names = Vec::new();
                    while self.peek().kind != TokenKind::RParen {
                        if self.at_eof() {
                            return Err(Error::parse(
                                "src.syntax",
                                start,
                                "unterminated export '('",
                            ));
                        }
                        let (name, _) = self.expect_ident("exported symbol name")?;
                        names.push(name);
                    }
                    self.advance();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::Export { names }),
                        1,
                    ));
                }
                "defn" => {
                    self.advance(); // defn
                    let (name, _) = self.expect_ident("function name")?;
                    let params = self.parse_param_list()?;
                    return self.start_nonempty_body(start, 0, BodyTarget::Defn { name, params });
                }
                "fn" => {
                    self.advance(); // fn
                    let params = self.parse_param_list()?;
                    return self.start_nonempty_body(start, 0, BodyTarget::Fn { params });
                }
                "let" => {
                    self.advance(); // let
                    let bind_start = self.expect(TokenKind::LParen, "let bindings '('")?;
                    if self.peek().kind == TokenKind::RParen {
                        self.advance();
                        return self.start_nonempty_body(
                            start,
                            0,
                            BodyTarget::Let {
                                bindings: Vec::new(),
                            },
                        );
                    }
                    let (key, _) = self.expect_ident("binding name")?;
                    self.frames
                        .push(ParseFrame::LetBindVal(Box::new(LetBindFrame {
                            start,
                            bind_start,
                            bindings: Vec::new(),
                            key,
                            max_child: 0,
                        })));
                    return Ok(NextStep::StartExpr);
                }
                "if" => {
                    self.advance(); // if
                    self.frames.push(ParseFrame::IfCond { start });
                    return Ok(NextStep::StartExpr);
                }
                "when" => {
                    self.advance(); // when
                    self.frames.push(ParseFrame::WhenCond { start });
                    return Ok(NextStep::StartExpr);
                }
                "with" => {
                    self.advance(); // with
                    self.frames.push(ParseFrame::WithTarget { start });
                    return Ok(NextStep::StartExpr);
                }
                "match" => {
                    self.advance(); // match
                    self.frames.push(ParseFrame::MatchScrutinee { start });
                    return Ok(NextStep::StartExpr);
                }
                _ => {}
            }
        }

        self.frames.push(ParseFrame::ParenFirst { start });
        Ok(NextStep::StartExpr)
    }

    pub(super) fn start_nonempty_body(
        &mut self,
        start: Span,
        max_child: usize,
        target: BodyTarget,
    ) -> Result<NextStep, Error> {
        if self.peek().kind == TokenKind::RParen {
            return Err(Error::parse("expr.body", start, "body cannot be empty"));
        }
        self.frames.push(ParseFrame::Body(Box::new(BodyFrame {
            start,
            body: Vec::new(),
            max_child,
            target,
        })));
        Ok(NextStep::StartExpr)
    }

    pub(super) fn finish_body(start: Span, body: Vec<Expr>, target: BodyTarget) -> Expr {
        let kind = match target {
            BodyTarget::Defn { name, params } => ExprKind::Defn { name, params, body },
            BodyTarget::Fn { params } => ExprKind::Fn { params, body },
            BodyTarget::Let { bindings } => ExprKind::Let { bindings, body },
            BodyTarget::When { cond } => ExprKind::When { cond, body },
        };
        Expr::new(start, kind)
    }

    pub(super) fn start_next_match_arm(
        &mut self,
        start: Span,
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        max_child: usize,
    ) -> Result<NextStep, Error> {
        if self.peek().kind == TokenKind::RParen {
            self.advance();
            return Ok(NextStep::HaveExpr(
                Expr::new(start, ExprKind::Match { scrutinee, arms }),
                1 + max_child,
            ));
        }
        if self.at_eof() {
            return Err(Error::parse("src.syntax", start, "unterminated match"));
        }
        let arm_start = self.expect(TokenKind::LParen, "match arm '('")?;
        let pattern = self.parse_pattern()?;
        if self.peek().kind == TokenKind::RParen {
            return Err(Error::parse("expr.body", arm_start, "body cannot be empty"));
        }
        self.frames
            .push(ParseFrame::MatchArmBody(Box::new(MatchArmFrame {
                start,
                scrutinee,
                arms,
                arm_start,
                pattern,
                body: Vec::new(),
                max_child,
            })));
        Ok(NextStep::StartExpr)
    }
}
