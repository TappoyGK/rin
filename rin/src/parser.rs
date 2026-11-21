//! 字句トークン列から AST を構築する。
//!
//! 深い入れ子でもネイティブスタックを消費しないよう、明示的スタックで構文解析する。

use crate::ast::{Expr, ExprKind, MatchArm, Pattern};
use crate::error::{Error, Span};
use crate::lexer::{Token, TokenKind, tokenize};

/// 構文解析および AST の入れ子深さの絶対上限。
pub(crate) const MAX_SYNTAX_NESTING: usize = 256;

/// Rin ソース文字列を解析し、トップレベル式の列を返す。
pub fn parse_program(input: &str) -> Result<Vec<Expr>, Error> {
    let tokens = tokenize(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    let mut exprs = Vec::new();
    while !parser.at_eof() {
        exprs.push(parser.parse_expr_iter()?);
    }
    Ok(exprs)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

#[derive(Clone, Copy)]
enum WrapKind {
    Some,
    Ok,
    Err,
}

enum BodyTarget {
    Defn { name: String, params: Vec<String> },
    Fn { params: Vec<String> },
    Let { bindings: Vec<(String, Expr)> },
    When { cond: Box<Expr> },
}

enum ParseFrame {
    PostfixIndex {
        target: Box<Expr>,
        target_depth: usize,
    },
    List {
        start: Span,
        items: Vec<Expr>,
        max_child: usize,
    },
    ParenFirst {
        start: Span,
    },
    Tuple {
        start: Span,
        items: Vec<Expr>,
        max_child: usize,
    },
    Call {
        start: Span,
        callee: Box<Expr>,
        args: Vec<Expr>,
        max_child: usize,
    },
    BuiltinWrap {
        span: Span,
        kind: WrapKind,
    },
    StructField {
        brace_start: Span,
        span: Span,
        name: String,
        fields: Vec<(String, Expr)>,
        key: String,
        max_child: usize,
    },
    VariantData {
        span: Span,
        type_name: String,
        variant: String,
    },
    Def {
        start: Span,
        name: String,
    },
    Body {
        start: Span,
        body: Vec<Expr>,
        max_child: usize,
        target: BodyTarget,
    },
    LetBindVal {
        start: Span,
        bind_start: Span,
        bindings: Vec<(String, Expr)>,
        key: String,
        max_child: usize,
    },
    IfCond {
        start: Span,
    },
    IfThen {
        start: Span,
        cond: Box<Expr>,
        max_child: usize,
    },
    IfElse {
        start: Span,
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        max_child: usize,
    },
    WhenCond {
        start: Span,
    },
    WithTarget {
        start: Span,
    },
    WithFieldVal {
        start: Span,
        brace_start: Span,
        target: Box<Expr>,
        fields: Vec<(String, Expr)>,
        key: String,
        max_child: usize,
    },
    MatchScrutinee {
        start: Span,
    },
    MatchArmBody {
        start: Span,
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        arm_start: Span,
        pattern: Pattern,
        body: Vec<Expr>,
        max_child: usize,
    },
}

enum NextStep {
    StartExpr,
    HaveExpr(Expr, usize),
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_next(&self) -> Option<&Token> {
        self.tokens.get(self.pos + 1)
    }

    fn at_eof(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    fn bump(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if !self.at_eof() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, expected: TokenKind, what: &str) -> Result<Token, Error> {
        let tok = self.peek().clone();
        if tok.kind == expected {
            self.pos += 1;
            Ok(tok)
        } else {
            Err(Error::parse(tok.span, format!("expected {what}")))
        }
    }

    fn expect_ident(&mut self, what: &str) -> Result<(String, Span), Error> {
        let tok = self.bump();
        match tok.kind {
            TokenKind::Ident(name) => Ok((name, tok.span)),
            _ => Err(Error::parse(tok.span, format!("expected {what}"))),
        }
    }

    fn check_nesting(depth: usize, span: Span) -> Result<(), Error> {
        if depth > MAX_SYNTAX_NESTING {
            return Err(Error::parse(
                span,
                format!("syntax nesting limit exceeded ({MAX_SYNTAX_NESTING})"),
            ));
        }
        Ok(())
    }

    fn parse_expr_iter(&mut self) -> Result<Expr, Error> {
        let mut frames: Vec<ParseFrame> = Vec::new();
        let mut next = NextStep::StartExpr;

        'outer: loop {
            match next {
                NextStep::StartExpr => {
                    let tok_span = self.peek().span;
                    Self::check_nesting(frames.len() + 1, tok_span)?;
                    next = self.start_primary(&mut frames)?;
                }
                NextStep::HaveExpr(mut expr, mut depth) => {
                    // 後置アクセス（`.` と隣接 `[`）はここで処理する。
                    loop {
                        let next_kind = &self.peek().kind;
                        let next_span = self.peek().span;
                        let next_attached = self.peek().attached;
                        if *next_kind == TokenKind::Dot {
                            Self::check_nesting(frames.len() + depth + 1, next_span)?;
                            self.bump();
                            let (field, _) = self.expect_ident("field name after '.'")?;
                            let span = expr.span;
                            expr = Expr::new(
                                span,
                                ExprKind::Field {
                                    target: Box::new(expr),
                                    field,
                                },
                            );
                            depth += 1;
                        } else if *next_kind == TokenKind::LBracket && next_attached {
                            Self::check_nesting(frames.len() + depth + 1, next_span)?;
                            self.bump();
                            frames.push(ParseFrame::PostfixIndex {
                                target: Box::new(expr),
                                target_depth: depth,
                            });
                            next = NextStep::StartExpr;
                            continue 'outer;
                        } else {
                            break;
                        }
                    }

                    let Some(frame) = frames.pop() else {
                        return Ok(expr);
                    };
                    next = self.resume_frame(frame, expr, depth, &mut frames)?;
                }
            }
        }
    }

    fn start_primary(&mut self, frames: &mut Vec<ParseFrame>) -> Result<NextStep, Error> {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::Int(n) => {
                self.bump();
                Ok(NextStep::HaveExpr(
                    Expr::new(tok.span, ExprKind::Int(*n)),
                    1,
                ))
            }
            TokenKind::Float(x) => {
                self.bump();
                Ok(NextStep::HaveExpr(
                    Expr::new(tok.span, ExprKind::Float(*x)),
                    1,
                ))
            }
            TokenKind::Str(s) => {
                self.bump();
                Ok(NextStep::HaveExpr(
                    Expr::new(tok.span, ExprKind::Str(s.clone())),
                    1,
                ))
            }
            TokenKind::Char(c) => {
                self.bump();
                Ok(NextStep::HaveExpr(
                    Expr::new(tok.span, ExprKind::Char(*c)),
                    1,
                ))
            }
            TokenKind::LBracket => {
                let start = self.bump().span;
                if self.peek().kind == TokenKind::RBracket {
                    self.bump();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::List(Vec::new())),
                        1,
                    ));
                }
                frames.push(ParseFrame::List {
                    start,
                    items: Vec::new(),
                    max_child: 0,
                });
                Ok(NextStep::StartExpr)
            }
            TokenKind::LParen => self.start_paren(frames),
            TokenKind::Ident(name) => self.start_ident(name.clone(), tok.span, frames),
            _ => Err(Error::parse(
                tok.span,
                format!("unexpected token {:?}", tok.kind),
            )),
        }
    }

    fn start_ident(
        &mut self,
        name: String,
        span: Span,
        frames: &mut Vec<ParseFrame>,
    ) -> Result<NextStep, Error> {
        self.bump();
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
                self.bump();
                frames.push(ParseFrame::BuiltinWrap {
                    span,
                    kind: WrapKind::Some,
                });
                return Ok(NextStep::StartExpr);
            }
            "ok" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                self.bump();
                frames.push(ParseFrame::BuiltinWrap {
                    span,
                    kind: WrapKind::Ok,
                });
                return Ok(NextStep::StartExpr);
            }
            "err" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                self.bump();
                frames.push(ParseFrame::BuiltinWrap {
                    span,
                    kind: WrapKind::Err,
                });
                return Ok(NextStep::StartExpr);
            }
            _ => {}
        }

        let is_upper = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        if is_upper && self.peek().kind == TokenKind::LBrace {
            let brace_start = self.bump().span;
            if self.peek().kind == TokenKind::RBrace {
                self.bump();
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
            frames.push(ParseFrame::StructField {
                brace_start,
                span,
                name,
                fields: Vec::new(),
                key,
                max_child: 0,
            });
            return Ok(NextStep::StartExpr);
        }

        if is_upper && self.peek().kind == TokenKind::Dot {
            self.bump();
            let (variant, _) = self.expect_ident("variant name after '.'")?;
            if self.peek().kind == TokenKind::LParen && self.peek().attached {
                self.bump();
                frames.push(ParseFrame::VariantData {
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

    fn start_paren(&mut self, frames: &mut Vec<ParseFrame>) -> Result<NextStep, Error> {
        let start = self.expect(TokenKind::LParen, "'('")?.span;
        if self.peek().kind == TokenKind::RParen {
            return Err(Error::parse(start, "empty '()' is not a valid expression"));
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
            match head.as_str() {
                "def" => {
                    self.bump(); // def
                    let (name, _) = self.expect_ident("variable name in def")?;
                    frames.push(ParseFrame::Def { start, name });
                    return Ok(NextStep::StartExpr);
                }
                "import" => {
                    self.bump(); // import
                    let (name, _) = self.expect_ident("module name")?;
                    self.expect(TokenKind::RParen, "')'")?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::Import { name }),
                        1,
                    ));
                }
                "export" => {
                    self.bump(); // export
                    let mut names = Vec::new();
                    while self.peek().kind != TokenKind::RParen {
                        if self.at_eof() {
                            return Err(Error::parse(start, "unterminated export '('"));
                        }
                        let (name, _) = self.expect_ident("exported symbol name")?;
                        names.push(name);
                    }
                    self.bump();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::Export { names }),
                        1,
                    ));
                }
                "defn" => {
                    self.bump(); // defn
                    let (name, _) = self.expect_ident("function name")?;
                    let params = self.parse_param_list()?;
                    if self.peek().kind == TokenKind::RParen {
                        self.bump();
                        return Ok(NextStep::HaveExpr(
                            Expr::new(
                                start,
                                ExprKind::Defn {
                                    name,
                                    params,
                                    body: Vec::new(),
                                },
                            ),
                            1,
                        ));
                    }
                    frames.push(ParseFrame::Body {
                        start,
                        body: Vec::new(),
                        max_child: 0,
                        target: BodyTarget::Defn { name, params },
                    });
                    return Ok(NextStep::StartExpr);
                }
                "fn" => {
                    self.bump(); // fn
                    let params = self.parse_param_list()?;
                    if self.peek().kind == TokenKind::RParen {
                        self.bump();
                        return Ok(NextStep::HaveExpr(
                            Expr::new(
                                start,
                                ExprKind::Fn {
                                    params,
                                    body: Vec::new(),
                                },
                            ),
                            1,
                        ));
                    }
                    frames.push(ParseFrame::Body {
                        start,
                        body: Vec::new(),
                        max_child: 0,
                        target: BodyTarget::Fn { params },
                    });
                    return Ok(NextStep::StartExpr);
                }
                "let" => {
                    self.bump(); // let
                    let bind_start = self.expect(TokenKind::LParen, "let bindings '('")?.span;
                    if self.peek().kind == TokenKind::RParen {
                        self.bump();
                        return self.start_body_or_finish(
                            start,
                            0,
                            BodyTarget::Let {
                                bindings: Vec::new(),
                            },
                            frames,
                        );
                    }
                    let (key, _) = self.expect_ident("binding name")?;
                    frames.push(ParseFrame::LetBindVal {
                        start,
                        bind_start,
                        bindings: Vec::new(),
                        key,
                        max_child: 0,
                    });
                    return Ok(NextStep::StartExpr);
                }
                "if" => {
                    self.bump(); // if
                    frames.push(ParseFrame::IfCond { start });
                    return Ok(NextStep::StartExpr);
                }
                "when" => {
                    self.bump(); // when
                    frames.push(ParseFrame::WhenCond { start });
                    return Ok(NextStep::StartExpr);
                }
                "with" => {
                    self.bump(); // with
                    frames.push(ParseFrame::WithTarget { start });
                    return Ok(NextStep::StartExpr);
                }
                "match" => {
                    self.bump(); // match
                    frames.push(ParseFrame::MatchScrutinee { start });
                    return Ok(NextStep::StartExpr);
                }
                _ => {}
            }
        }

        frames.push(ParseFrame::ParenFirst { start });
        Ok(NextStep::StartExpr)
    }

    fn start_body_or_finish(
        &mut self,
        start: Span,
        max_child: usize,
        target: BodyTarget,
        frames: &mut Vec<ParseFrame>,
    ) -> Result<NextStep, Error> {
        if self.peek().kind == TokenKind::RParen {
            self.bump();
            return Ok(NextStep::HaveExpr(
                Self::finish_body(start, Vec::new(), target),
                1 + max_child,
            ));
        }
        frames.push(ParseFrame::Body {
            start,
            body: Vec::new(),
            max_child,
            target,
        });
        Ok(NextStep::StartExpr)
    }

    fn finish_body(start: Span, body: Vec<Expr>, target: BodyTarget) -> Expr {
        let kind = match target {
            BodyTarget::Defn { name, params } => ExprKind::Defn { name, params, body },
            BodyTarget::Fn { params } => ExprKind::Fn { params, body },
            BodyTarget::Let { bindings } => ExprKind::Let { bindings, body },
            BodyTarget::When { cond } => ExprKind::When { cond, body },
        };
        Expr::new(start, kind)
    }

    fn start_next_match_arm(
        &mut self,
        start: Span,
        scrutinee: Box<Expr>,
        mut arms: Vec<MatchArm>,
        max_child: usize,
        frames: &mut Vec<ParseFrame>,
    ) -> Result<NextStep, Error> {
        loop {
            if self.peek().kind == TokenKind::RParen {
                self.bump();
                return Ok(NextStep::HaveExpr(
                    Expr::new(start, ExprKind::Match { scrutinee, arms }),
                    1 + max_child,
                ));
            }
            if self.at_eof() {
                return Err(Error::parse(start, "unterminated match"));
            }
            let arm_start = self.expect(TokenKind::LParen, "match arm '('")?.span;
            let pattern = self.parse_pattern()?;
            if self.peek().kind == TokenKind::RParen {
                self.bump();
                arms.push(MatchArm {
                    span: arm_start,
                    pattern,
                    body: Vec::new(),
                });
                continue;
            }
            frames.push(ParseFrame::MatchArmBody {
                start,
                scrutinee,
                arms,
                arm_start,
                pattern,
                body: Vec::new(),
                max_child,
            });
            return Ok(NextStep::StartExpr);
        }
    }

    fn resume_frame(
        &mut self,
        frame: ParseFrame,
        expr: Expr,
        depth: usize,
        frames: &mut Vec<ParseFrame>,
    ) -> Result<NextStep, Error> {
        match frame {
            ParseFrame::PostfixIndex {
                target,
                target_depth,
            } => {
                self.expect(TokenKind::RBracket, "']'")?;
                let combined = 1 + target_depth.max(depth);
                Self::check_nesting(frames.len() + combined, target.span)?;
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
                    self.bump();
                }
                if self.peek().kind == TokenKind::RBracket {
                    self.bump();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::List(items)),
                        1 + max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(Error::parse(start, "unterminated '['"));
                }
                frames.push(ParseFrame::List {
                    start,
                    items,
                    max_child,
                });
                Ok(NextStep::StartExpr)
            }
            ParseFrame::ParenFirst { start } => {
                if self.peek().kind == TokenKind::Comma {
                    self.bump();
                    let items = vec![expr];
                    if self.peek().kind == TokenKind::RParen {
                        self.bump();
                        return Ok(NextStep::HaveExpr(
                            Expr::new(start, ExprKind::Tuple(items)),
                            1 + depth,
                        ));
                    }
                    if self.at_eof() {
                        return Err(Error::parse(start, "unterminated tuple '('"));
                    }
                    frames.push(ParseFrame::Tuple {
                        start,
                        items,
                        max_child: depth,
                    });
                    return Ok(NextStep::StartExpr);
                }
                if self.peek().kind == TokenKind::RParen {
                    self.bump();
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
                    return Err(Error::parse(start, "unterminated '('"));
                }
                frames.push(ParseFrame::Call {
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
                    self.bump();
                    if self.peek().kind == TokenKind::RParen {
                        self.bump();
                        return Ok(NextStep::HaveExpr(
                            Expr::new(start, ExprKind::Tuple(items)),
                            1 + max_child,
                        ));
                    }
                    if self.at_eof() {
                        return Err(Error::parse(start, "unterminated tuple '('"));
                    }
                    frames.push(ParseFrame::Tuple {
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
                    self.bump();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::Call { callee, args }),
                        1 + max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(Error::parse(start, "unterminated '('"));
                }
                frames.push(ParseFrame::Call {
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
            ParseFrame::StructField {
                brace_start,
                span,
                name,
                mut fields,
                key,
                mut max_child,
            } => {
                fields.push((key, expr));
                max_child = max_child.max(depth);
                if self.peek().kind == TokenKind::Comma {
                    self.bump();
                }
                if self.peek().kind == TokenKind::RBrace {
                    self.bump();
                    return Ok(NextStep::HaveExpr(
                        Expr::new(span, ExprKind::Struct { name, fields }),
                        1 + max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(Error::parse(brace_start, "unterminated '{'"));
                }
                let (next_key, _) = self.expect_ident("field name")?;
                self.expect(TokenKind::Colon, "':'")?;
                frames.push(ParseFrame::StructField {
                    brace_start,
                    span,
                    name,
                    fields,
                    key: next_key,
                    max_child,
                });
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
            ParseFrame::Body {
                start,
                mut body,
                mut max_child,
                target,
            } => {
                body.push(expr);
                max_child = max_child.max(depth);
                if self.peek().kind == TokenKind::RParen {
                    self.bump();
                    return Ok(NextStep::HaveExpr(
                        Self::finish_body(start, body, target),
                        1 + max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(Error::parse(start, "unterminated '('"));
                }
                frames.push(ParseFrame::Body {
                    start,
                    body,
                    max_child,
                    target,
                });
                Ok(NextStep::StartExpr)
            }
            ParseFrame::LetBindVal {
                start,
                bind_start,
                mut bindings,
                key,
                mut max_child,
            } => {
                bindings.push((key, expr));
                max_child = max_child.max(depth);
                if self.peek().kind == TokenKind::RParen {
                    self.bump();
                    return self.start_body_or_finish(
                        start,
                        max_child,
                        BodyTarget::Let { bindings },
                        frames,
                    );
                }
                if self.at_eof() {
                    return Err(Error::parse(bind_start, "unterminated let bindings"));
                }
                let (next_key, _) = self.expect_ident("binding name")?;
                frames.push(ParseFrame::LetBindVal {
                    start,
                    bind_start,
                    bindings,
                    key: next_key,
                    max_child,
                });
                Ok(NextStep::StartExpr)
            }
            ParseFrame::IfCond { start } => {
                frames.push(ParseFrame::IfThen {
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
                        start,
                        "'if' requires 3 arguments (cond, then, else); use 'when' when no value is needed",
                    ));
                }
                frames.push(ParseFrame::IfElse {
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
            ParseFrame::WhenCond { start } => self.start_body_or_finish(
                start,
                depth,
                BodyTarget::When {
                    cond: Box::new(expr),
                },
                frames,
            ),
            ParseFrame::WithTarget { start } => {
                let brace_start = self.expect(TokenKind::LBrace, "'{'")?.span;
                let target = Box::new(expr);
                if self.peek().kind == TokenKind::RBrace {
                    self.bump();
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
                frames.push(ParseFrame::WithFieldVal {
                    start,
                    brace_start,
                    target,
                    fields: Vec::new(),
                    key,
                    max_child: depth,
                });
                Ok(NextStep::StartExpr)
            }
            ParseFrame::WithFieldVal {
                start,
                brace_start,
                target,
                mut fields,
                key,
                mut max_child,
            } => {
                fields.push((key, expr));
                max_child = max_child.max(depth);
                if self.peek().kind == TokenKind::Comma {
                    self.bump();
                }
                if self.peek().kind == TokenKind::RBrace {
                    self.bump();
                    self.expect(TokenKind::RParen, "')'")?;
                    return Ok(NextStep::HaveExpr(
                        Expr::new(start, ExprKind::With { target, fields }),
                        1 + max_child,
                    ));
                }
                if self.at_eof() {
                    return Err(Error::parse(brace_start, "unterminated '{'"));
                }
                let (next_key, _) = self.expect_ident("field name")?;
                self.expect(TokenKind::Colon, "':'")?;
                frames.push(ParseFrame::WithFieldVal {
                    start,
                    brace_start,
                    target,
                    fields,
                    key: next_key,
                    max_child,
                });
                Ok(NextStep::StartExpr)
            }
            ParseFrame::MatchScrutinee { start } => {
                self.start_next_match_arm(start, Box::new(expr), Vec::new(), depth, frames)
            }
            ParseFrame::MatchArmBody {
                start,
                scrutinee,
                mut arms,
                arm_start,
                pattern,
                mut body,
                mut max_child,
            } => {
                body.push(expr);
                max_child = max_child.max(depth);
                if self.peek().kind == TokenKind::RParen {
                    self.bump();
                    arms.push(MatchArm {
                        span: arm_start,
                        pattern,
                        body,
                    });
                    return self.start_next_match_arm(start, scrutinee, arms, max_child, frames);
                }
                if self.at_eof() {
                    return Err(Error::parse(arm_start, "unterminated '('"));
                }
                frames.push(ParseFrame::MatchArmBody {
                    start,
                    scrutinee,
                    arms,
                    arm_start,
                    pattern,
                    body,
                    max_child,
                });
                Ok(NextStep::StartExpr)
            }
        }
    }

    fn parse_param_list(&mut self) -> Result<Vec<String>, Error> {
        let start = self.expect(TokenKind::LParen, "parameter list '('")?.span;
        let mut params = Vec::new();
        while self.peek().kind != TokenKind::RParen {
            if self.at_eof() {
                return Err(Error::parse(start, "unterminated parameter list"));
            }
            let (param, _) = self.expect_ident("parameter name")?;
            params.push(param);
        }
        self.bump();
        Ok(params)
    }

    fn parse_pattern(&mut self) -> Result<Pattern, Error> {
        let tok = self.bump();
        match tok.kind {
            TokenKind::Int(n) => Ok(Pattern::Int(n)),
            TokenKind::Str(s) => Ok(Pattern::Str(s)),
            TokenKind::Char(c) => Ok(Pattern::Char(c)),
            TokenKind::Ident(name) => match name.as_str() {
                "_" => Ok(Pattern::Wildcard),
                "true" => Ok(Pattern::Bool(true)),
                "false" => Ok(Pattern::Bool(false)),
                "none" => Ok(Pattern::None),
                "some" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::Some(self.parse_opt_binding()?))
                }
                "ok" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::Ok(self.parse_opt_binding()?))
                }
                "err" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::Err(self.parse_opt_binding()?))
                }
                _ if name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) => {
                    self.expect(TokenKind::Dot, "'.' in variant pattern")?;
                    let (variant, _) = self.expect_ident("variant name")?;
                    let binding = if self.peek().kind == TokenKind::LParen && self.peek().attached {
                        self.parse_opt_binding()?
                    } else {
                        None
                    };
                    Ok(Pattern::Variant {
                        type_name: name,
                        variant,
                        binding,
                    })
                }
                _ => Ok(Pattern::Var(name)),
            },
            _ => Err(Error::parse(tok.span, "invalid match pattern")),
        }
    }

    fn parse_opt_binding(&mut self) -> Result<Option<String>, Error> {
        self.expect(TokenKind::LParen, "'('")?;
        let (name, _) = self.expect_ident("binding name or '_'")?;
        self.expect(TokenKind::RParen, "')'")?;
        Ok(if name == "_" { None } else { Some(name) })
    }
}
