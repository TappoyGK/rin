//! 字句トークン列から作業領域 (`WorkArena`) 上の AST を構築する。
//!
//! 深い入れ子でもネイティブスタックを消費しないよう、明示的スタックで構文解析する。

mod frames;
mod pattern;
mod primary;
mod resume;
use frames::*;

#[cfg(feature = "std-value")]
use crate::alloc::HeapAllocator;
use crate::ast::{
    DefnExpr, Expr, ExprKind, FnExpr, LetExpr, MatchArm, Pattern, StructExpr, VariantExpr,
};
#[cfg(feature = "std-value")]
use crate::error::Error;
use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::lexer::{WorkLexer, WorkToken, WorkTokenKind};
#[cfg(feature = "std-value")]
use crate::mem::RegionConfig;
use crate::mem::{AllocCore, RawStr, RawVec, WorkArena, WorkBox};

/// 構文解析および AST の入れ子深さの絶対上限。
pub(crate) const MAX_SYNTAX_NESTING: usize = 256;

/// `fmt` や `test` など `Pool` の外から構文木を所有するためのラッパー。
#[cfg(feature = "std-value")]
pub(crate) struct OwnedAst {
    work: WorkArena,
    exprs: RawVec<Expr>,
}

#[cfg(feature = "std-value")]
impl OwnedAst {
    pub(crate) fn exprs(&self) -> &[Expr] {
        self.exprs.as_slice()
    }
}

#[cfg(feature = "std-value")]
impl Drop for OwnedAst {
    fn drop(&mut self) {
        let mut alloc = HeapAllocator;
        let mut core = AllocCore::new(&mut alloc, usize::MAX);
        self.work.destroy(&mut core);
    }
}

/// 作業領域 `WorkArena` 上へ Rin ソース文字列を解析する（システムヒープ不使用・ストリーミング字句解析）。
pub(crate) fn parse_program_work(
    input: &str,
    work: &mut WorkArena,
    core: &mut AllocCore<'_>,
    test_mode: bool,
) -> Result<RawVec<Expr>, ErrRecorded> {
    let mut lexer = WorkLexer::new(input);
    let first = lexer.next_token(work, core)?;
    let mut frames = RawVec::<ParseFrame>::new();
    let mut exprs = RawVec::<Expr>::new();
    let mut parser = Parser {
        test_mode,
        lexer,
        cur: first,
        next: None,
        frames: &mut frames,
        work,
        core,
    };
    while !parser.at_eof() {
        let expr = parser.parse_expr_iter()?;
        if exprs.push_work(parser.work, parser.core, expr).is_none() {
            return Err(parser.oom(expr.span));
        }
    }
    Ok(exprs)
}

#[cfg(feature = "std-value")]
pub(crate) fn parse_tests(input: &str) -> Result<OwnedAst, Error> {
    parse_owned(input, true)
}

#[cfg(feature = "std-value")]
pub(crate) fn parse_owned(input: &str, test_mode: bool) -> Result<OwnedAst, Error> {
    let mut alloc = HeapAllocator;
    let mut core = AllocCore::new(&mut alloc, usize::MAX);
    let mut work = WorkArena::new(RegionConfig::new(4 * 1024, 64 * 1024));
    match parse_program_work(input, &mut work, &mut core, test_mode) {
        Ok(exprs) => Ok(OwnedAst { work, exprs }),
        Err(_) => {
            let err = core.error.to_owned_error();
            work.destroy(&mut core);
            Err(err)
        }
    }
}

struct Parser<'s, 'f, 'c> {
    test_mode: bool,
    lexer: WorkLexer<'s>,
    cur: WorkToken,
    next: Option<WorkToken>,
    frames: &'f mut RawVec<ParseFrame>,
    work: &'f mut WorkArena,
    core: &'f mut AllocCore<'c>,
}

#[cfg(test)]
pub(crate) fn parse_frame_size() -> usize {
    std::mem::size_of::<ParseFrame>()
}

enum NextStep {
    StartExpr,
    HaveExpr(Expr, usize),
}

impl Parser<'_, '_, '_> {
    #[cold]
    pub(super) fn oom(&mut self, span: Span) -> ErrRecorded {
        self.core.err_oom(span)
    }

    #[cold]
    pub(super) fn parse_err(&mut self, rule: &'static str, span: Span, msg: &str) -> ErrRecorded {
        self.core.err_str(rule, ErrorKind::Parse, span, msg)
    }

    #[cold]
    pub(super) fn parse_err_fmt(
        &mut self,
        rule: &'static str,
        span: Span,
        args: std::fmt::Arguments<'_>,
    ) -> ErrRecorded {
        self.core.err_fmt(rule, ErrorKind::Parse, span, args)
    }

    pub(super) fn push_frame(&mut self, frame: ParseFrame, span: Span) -> Result<(), ErrRecorded> {
        if self.frames.push_work(self.work, self.core, frame).is_none() {
            return Err(self.oom(span));
        }
        Ok(())
    }

    pub(super) fn alloc_box<T>(&mut self, val: T, span: Span) -> Result<WorkBox<T>, ErrRecorded> {
        match WorkBox::new(self.work, self.core, val) {
            Some(b) => Ok(b),
            None => Err(self.oom(span)),
        }
    }

    #[inline(always)]
    fn peek(&self) -> WorkToken {
        self.cur
    }

    #[inline(always)]
    fn peek_next(&mut self) -> Result<Option<WorkToken>, ErrRecorded> {
        if self.cur.kind == WorkTokenKind::Eof {
            return Ok(None);
        }
        if self.next.is_none() {
            self.next = Some(self.lexer.next_token(self.work, self.core)?);
        }
        Ok(self.next)
    }

    #[inline(always)]
    fn at_eof(&self) -> bool {
        self.cur.kind == WorkTokenKind::Eof
    }

    #[inline(always)]
    fn advance(&mut self) -> Result<(), ErrRecorded> {
        if self.cur.kind != WorkTokenKind::Eof {
            self.cur = match self.next.take() {
                Some(tok) => tok,
                None => self.lexer.next_token(self.work, self.core)?,
            };
        }
        Ok(())
    }

    fn bump(&mut self) -> Result<WorkToken, ErrRecorded> {
        let tok = self.cur;
        self.advance()?;
        Ok(tok)
    }

    fn expect(&mut self, expected: WorkTokenKind, what: &str) -> Result<Span, ErrRecorded> {
        let tok = self.peek();
        if tok.kind == expected {
            let span = tok.span;
            self.advance()?;
            Ok(span)
        } else {
            Err(self.parse_err_fmt("src.syntax", tok.span, format_args!("expected {what}")))
        }
    }

    fn expect_ident(&mut self, what: &str) -> Result<(RawStr, Span), ErrRecorded> {
        let tok = self.bump()?;
        match tok.kind {
            WorkTokenKind::Ident(name) => Ok((name, tok.span)),
            _ => Err(self.parse_err_fmt("src.syntax", tok.span, format_args!("expected {what}"))),
        }
    }

    fn check_nesting(&mut self, depth: usize, span: Span) -> Result<(), ErrRecorded> {
        if depth > MAX_SYNTAX_NESTING {
            return Err(self.parse_err_fmt(
                "src.nesting-limit",
                span,
                format_args!("syntax nesting limit exceeded ({MAX_SYNTAX_NESTING})"),
            ));
        }
        Ok(())
    }

    fn parse_expr_iter(&mut self) -> Result<Expr, ErrRecorded> {
        self.frames.clear();
        let mut next = NextStep::StartExpr;

        'outer: loop {
            match next {
                NextStep::StartExpr => {
                    let tok_span = self.peek().span;
                    self.check_nesting(self.frames.len() + 1, tok_span)?;
                    next = self.start_primary()?;
                }
                NextStep::HaveExpr(mut expr, mut depth) => {
                    // 後置アクセス（`.` と隣接 `[`）はここで処理する。
                    loop {
                        let next_tok = self.peek();
                        if next_tok.kind == WorkTokenKind::Dot {
                            self.check_nesting(self.frames.len() + depth + 1, next_tok.span)?;
                            self.advance()?;
                            let (field, _) = self.expect_ident("field name after '.'")?;
                            let span = expr.span;
                            let target = self.alloc_box(expr, span)?;
                            expr = Expr::new(span, ExprKind::Field { target, field });
                            depth += 1;
                        } else if next_tok.kind == WorkTokenKind::LBracket && next_tok.attached {
                            self.check_nesting(self.frames.len() + depth + 1, next_tok.span)?;
                            self.advance()?;
                            let target = self.alloc_box(expr, next_tok.span)?;
                            self.push_frame(
                                ParseFrame::PostfixIndex {
                                    target,
                                    target_depth: depth,
                                },
                                next_tok.span,
                            )?;
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
