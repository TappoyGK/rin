//! 字句トークン列から AST を構築する。
//!
//! 深い入れ子でもネイティブスタックを消費しないよう、明示的スタックで構文解析する。

use crate::ast::{Expr, ExprKind, MatchArm, Pattern};
use crate::error::{Error, Span};
use crate::lexer::{Token, TokenKind, tokenize_into};

/// 構文解析および AST の入れ子深さの絶対上限。
pub(crate) const MAX_SYNTAX_NESTING: usize = 256;

/// 構文解析器が再利用する作業バッファ。
///
/// 小さな入力ではバッファを再利用し、評価終了時（エラー時を含む）に即座に中身と過大領域を解放する。
pub(crate) struct ParseScratch {
    tokens: Vec<Token>,
    frames: Vec<ParseFrame>,
    exprs: Vec<Expr>,
}

impl ParseScratch {
    pub(crate) fn new() -> Self {
        Self {
            tokens: Vec::new(),
            frames: Vec::new(),
            exprs: Vec::new(),
        }
    }

    /// 評価の終了時（エラー時を含む）に AST・トークン列を即座に解放し、過大なバッファを縮退させる。
    pub(crate) fn reset_after_eval(&mut self) {
        self.exprs.clear();
        if self.exprs.capacity() > 16 {
            self.exprs = Vec::new();
        }
        self.tokens.clear();
        if self.tokens.capacity() > 64 {
            self.tokens = Vec::new();
        }
        self.frames.clear();
        if self.frames.capacity() > 32 {
            self.frames = Vec::new();
        }
    }

    #[cfg(test)]
    pub(crate) fn capacities(&self) -> (usize, usize, usize) {
        (
            self.tokens.capacity(),
            self.frames.capacity(),
            self.exprs.capacity(),
        )
    }
}

/// 評価スコープ終了時に `ParseScratch` を確実に解放する RAII ガード。
pub(crate) struct ParseScratchGuard<'a>(&'a mut ParseScratch);

impl<'a> ParseScratchGuard<'a> {
    pub(crate) fn new(scratch: &'a mut ParseScratch) -> Self {
        Self(scratch)
    }

    pub(crate) fn parse<'s>(&'s mut self, input: &str) -> Result<&'s [Expr], Error> {
        parse_program_with_scratch(input, self.0)
    }
}

impl Drop for ParseScratchGuard<'_> {
    fn drop(&mut self) {
        self.0.reset_after_eval();
    }
}

/// 作業バッファを再利用して Rin ソース文字列を解析する。
pub(crate) fn parse_program_with_scratch<'s>(
    input: &str,
    scratch: &'s mut ParseScratch,
) -> Result<&'s [Expr], Error> {
    tokenize_into(input, &mut scratch.tokens)?;
    scratch.exprs.clear();
    let mut parser = Parser {
        tokens: &scratch.tokens,
        pos: 0,
        frames: &mut scratch.frames,
    };
    while !parser.at_eof() {
        scratch.exprs.push(parser.parse_expr_iter()?);
    }
    Ok(&scratch.exprs)
}

struct Parser<'f> {
    tokens: &'f [Token],
    pos: usize,
    frames: &'f mut Vec<ParseFrame>,
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

struct StructFieldFrame {
    brace_start: Span,
    span: Span,
    name: String,
    fields: Vec<(String, Expr)>,
    key: String,
    max_child: usize,
}

struct BodyFrame {
    start: Span,
    body: Vec<Expr>,
    max_child: usize,
    target: BodyTarget,
}

struct LetBindFrame {
    start: Span,
    bind_start: Span,
    bindings: Vec<(String, Expr)>,
    key: String,
    max_child: usize,
}

struct WithFieldFrame {
    start: Span,
    brace_start: Span,
    target: Box<Expr>,
    fields: Vec<(String, Expr)>,
    key: String,
    max_child: usize,
}

struct MatchArmFrame {
    start: Span,
    scrutinee: Box<Expr>,
    arms: Vec<MatchArm>,
    arm_start: Span,
    pattern: Pattern,
    body: Vec<Expr>,
    max_child: usize,
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
    StructField(Box<StructFieldFrame>),
    VariantData {
        span: Span,
        type_name: String,
        variant: String,
    },
    Def {
        start: Span,
        name: String,
    },
    Body(Box<BodyFrame>),
    LetBindVal(Box<LetBindFrame>),
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
    WithFieldVal(Box<WithFieldFrame>),
    MatchScrutinee {
        start: Span,
    },
    MatchArmBody(Box<MatchArmFrame>),
}

#[cfg(test)]
pub(crate) fn parse_frame_size() -> usize {
    std::mem::size_of::<ParseFrame>()
}

enum NextStep {
    StartExpr,
    HaveExpr(Expr, usize),
}

impl Parser<'_> {
    #[inline(always)]
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    #[inline(always)]
    fn peek_next(&self) -> Option<&Token> {
        self.tokens.get(self.pos + 1)
    }

    #[inline(always)]
    fn at_eof(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    #[inline(always)]
    fn advance(&mut self) {
        if !self.at_eof() {
            self.pos += 1;
        }
    }

    fn bump(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        self.advance();
        tok
    }

    fn expect(&mut self, expected: TokenKind, what: &str) -> Result<Span, Error> {
        let tok = self.peek();
        if tok.kind == expected {
            let span = tok.span;
            self.advance();
            Ok(span)
        } else {
            Err(Error::parse(
                "src.syntax",
                tok.span,
                format!("expected {what}"),
            ))
        }
    }

    fn expect_ident(&mut self, what: &str) -> Result<(String, Span), Error> {
        let tok = self.bump();
        match tok.kind {
            TokenKind::Ident(name) => Ok((name, tok.span)),
            _ => Err(Error::parse(
                "src.syntax",
                tok.span,
                format!("expected {what}"),
            )),
        }
    }

    fn check_nesting(depth: usize, span: Span) -> Result<(), Error> {
        if depth > MAX_SYNTAX_NESTING {
            return Err(Error::parse(
                "src.nesting-limit",
                span,
                format!("syntax nesting limit exceeded ({MAX_SYNTAX_NESTING})"),
            ));
        }
        Ok(())
    }

    fn parse_expr_iter(&mut self) -> Result<Expr, Error> {
        self.frames.clear();
        let mut next = NextStep::StartExpr;

        'outer: loop {
            match next {
                NextStep::StartExpr => {
                    let tok_span = self.peek().span;
                    Self::check_nesting(self.frames.len() + 1, tok_span)?;
                    next = self.start_primary()?;
                }
                NextStep::HaveExpr(mut expr, mut depth) => {
                    // 後置アクセス（`.` と隣接 `[`）はここで処理する。
                    loop {
                        let next_kind = &self.peek().kind;
                        let next_span = self.peek().span;
                        let next_attached = self.peek().attached;
                        if *next_kind == TokenKind::Dot {
                            Self::check_nesting(self.frames.len() + depth + 1, next_span)?;
                            self.advance();
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
                            Self::check_nesting(self.frames.len() + depth + 1, next_span)?;
                            self.advance();
                            self.frames.push(ParseFrame::PostfixIndex {
                                target: Box::new(expr),
                                target_depth: depth,
                            });
                            next = NextStep::StartExpr;
                            continue 'outer;
                        } else {
                            break;
                        }
                    }

                    let Some(frame) = self.frames.pop() else {
                        return Ok(expr);
                    };
                    next = self.resume_frame(frame, expr, depth)?;
                }
            }
        }
    }

    fn start_primary(&mut self) -> Result<NextStep, Error> {
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

    fn start_nonempty_body(
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

    fn resume_frame(
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

    fn parse_param_list(&mut self) -> Result<Vec<String>, Error> {
        let start = self.expect(TokenKind::LParen, "parameter list '('")?;
        let mut params = Vec::new();
        while self.peek().kind != TokenKind::RParen {
            if self.at_eof() {
                return Err(Error::parse(
                    "src.syntax",
                    start,
                    "unterminated parameter list",
                ));
            }
            let (param, _) = self.expect_ident("parameter name")?;
            params.push(param);
        }
        self.advance();
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
            _ => Err(Error::parse("pat.kinds", tok.span, "invalid match pattern")),
        }
    }

    fn parse_opt_binding(&mut self) -> Result<Option<String>, Error> {
        self.expect(TokenKind::LParen, "'('")?;
        let (name, _) = self.expect_ident("binding name or '_'")?;
        self.expect(TokenKind::RParen, "')'")?;
        Ok(if name == "_" { None } else { Some(name) })
    }
}
