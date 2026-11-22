//! 字句トークン列から AST を構築する。
//!
//! 深い入れ子でもネイティブスタックを消費しないよう、明示的スタックで構文解析する。

mod frames;
mod pattern;
mod primary;
mod resume;
use frames::*;

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
    parse_with_mode(input, scratch, false)
}

pub(crate) fn parse_tests(input: &str) -> Result<Vec<Expr>, Error> {
    parse_owned(input, true)
}

pub(crate) fn parse_owned(input: &str, test_mode: bool) -> Result<Vec<Expr>, Error> {
    let mut scratch = ParseScratch::new();
    parse_with_mode(input, &mut scratch, test_mode)?;
    Ok(std::mem::take(&mut scratch.exprs))
}

fn parse_with_mode<'s>(
    input: &str,
    scratch: &'s mut ParseScratch,
    test_mode: bool,
) -> Result<&'s [Expr], Error> {
    tokenize_into(input, &mut scratch.tokens)?;
    scratch.exprs.clear();
    let mut parser = Parser {
        tokens: &scratch.tokens,
        pos: 0,
        frames: &mut scratch.frames,
        test_mode,
    };
    while !parser.at_eof() {
        scratch.exprs.push(parser.parse_expr_iter()?);
    }
    Ok(&scratch.exprs)
}

struct Parser<'f> {
    test_mode: bool,
    tokens: &'f [Token],
    pos: usize,
    frames: &'f mut Vec<ParseFrame>,
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
}
