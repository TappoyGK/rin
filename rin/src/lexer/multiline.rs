//! 複数行文字列リテラル (`"""..."""`) の閉じ引用符先読み・インデント基準解決ヘルパ。

use crate::error::Span;

use super::{Lexer, MAX_TEMPLATE_NESTING};

/// 複数行文字列リテラルのインデント基準と閉じ行の位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MultilineCtx {
    pub(super) indent_start: usize,
    pub(super) indent_len: usize,
    pub(super) closing_line_start: usize,
}

/// テンプレート埋め込み `${...}` の波括弧ネスト管理エントリ。
#[derive(Debug, Clone, Copy)]
pub(super) struct BraceEntry {
    pub(super) braces: u16,
    pub(super) multiline: Option<MultilineCtx>,
}

impl BraceEntry {
    #[inline]
    pub(super) const fn single() -> Self {
        Self {
            braces: 0,
            multiline: None,
        }
    }

    #[inline]
    pub(super) const fn multiline(ctx: MultilineCtx) -> Self {
        Self {
            braces: 0,
            multiline: Some(ctx),
        }
    }
}

pub(super) enum MultilineScanErr {
    Unterminated(Span),
    LoneCr(Span),
    MisplacedClosing(Span),
    MissingNewlineAfterOpen(Span),
    RawNewlineInInnerString(Span),
    NestingLimit(Span),
}

#[derive(Clone, Copy)]
enum SkipFrame {
    Expr { braces: u16 },
    SingleStr,
    MultiStr { at_line_start: bool },
}

impl Lexer<'_> {
    /// 開始 `"""` の直後の必須改行（LF または CRLF）を消費する。
    pub(super) fn consume_multiline_open_newline(&mut self, open_span: Span) -> Result<(), Span> {
        match self.peek() {
            Some(b'\n') => {
                self.bump();
                Ok(())
            }
            Some(b'\r') if self.peek_at(1) == Some(b'\n') => {
                self.bump();
                self.bump();
                Ok(())
            }
            _ => Err(open_span),
        }
    }

    /// 開始 `"""` と直後の改行を消費した直後の位置から先読みし、
    /// 対応する閉じ `"""` の行頭位置と基準インデントを求める。
    pub(super) fn scan_multiline_closing(
        &self,
        open_span: Span,
    ) -> Result<MultilineCtx, MultilineScanErr> {
        let src = self.src;
        let mut p = self.pos;
        let mut line = self.line;
        let mut col = self.col;
        let mut line_start = p;
        let mut at_line_start = true;

        while p < src.len() {
            let b = src[p];
            if at_line_start && (b == b' ' || b == b'\t') {
                p += 1;
                col += 1;
                continue;
            }
            if b == b'"' && src.get(p + 1) == Some(&b'"') && src.get(p + 2) == Some(&b'"') {
                if at_line_start {
                    return Ok(MultilineCtx {
                        indent_start: line_start,
                        indent_len: p - line_start,
                        closing_line_start: line_start,
                    });
                }
                return Err(MultilineScanErr::MisplacedClosing(Span::new(line, col)));
            }
            if b == b'\n' {
                p += 1;
                line += 1;
                col = 1;
                line_start = p;
                at_line_start = true;
                continue;
            }
            if b == b'\r' {
                if src.get(p + 1) == Some(&b'\n') {
                    p += 2;
                    line += 1;
                    col = 1;
                    line_start = p;
                    at_line_start = true;
                    continue;
                }
                return Err(MultilineScanErr::LoneCr(Span::new(line, col)));
            }
            at_line_start = false;
            if b == b'\\' {
                p += 1;
                col += 1;
                if p < src.len() && src[p] != b'\n' && src[p] != b'\r' {
                    p += 1;
                    col += 1;
                }
                continue;
            }
            if b == b'$' && src.get(p + 1) == Some(&b'{') {
                p += 2;
                col += 2;
                skip_interpolation_expr(src, &mut p, &mut line, &mut col, open_span)?;
                continue;
            }
            p += 1;
            col += 1;
        }
        Err(MultilineScanErr::Unterminated(open_span))
    }
}

/// `${` の直後から対応する `}` までをスキップする（入れ子の文字列・コメント・文字リテラルを考慮）。
fn skip_interpolation_expr(
    src: &[u8],
    p: &mut usize,
    line: &mut usize,
    col: &mut usize,
    outer_open: Span,
) -> Result<(), MultilineScanErr> {
    let mut stack = [SkipFrame::Expr { braces: 0 }; MAX_TEMPLATE_NESTING + 4];
    let mut sp = 1usize;

    while sp > 0 && *p < src.len() {
        match stack[sp - 1] {
            SkipFrame::Expr { mut braces } => {
                let b = src[*p];
                match b {
                    b';' => {
                        while *p < src.len() && src[*p] != b'\n' {
                            *p += 1;
                            *col += 1;
                        }
                    }
                    b'\\' => {
                        // 文字リテラル（\"・\{・\}・\;・\u{...} 等）
                        *p += 1;
                        *col += 1;
                        if src.get(*p) == Some(&b'u') && src.get(*p + 1) == Some(&b'{') {
                            *p += 2;
                            *col += 2;
                            while *p < src.len() && src[*p] != b'}' && src[*p] != b'\n' {
                                *p += 1;
                                *col += 1;
                            }
                            if src.get(*p) == Some(&b'}') {
                                *p += 1;
                                *col += 1;
                            }
                        } else if *p < src.len() && src[*p] != b'\n' {
                            *p += 1;
                            *col += 1;
                        }
                    }
                    b'{' => {
                        braces = braces.saturating_add(1);
                        stack[sp - 1] = SkipFrame::Expr { braces };
                        *p += 1;
                        *col += 1;
                    }
                    b'}' => {
                        *p += 1;
                        *col += 1;
                        if braces > 0 {
                            braces -= 1;
                            stack[sp - 1] = SkipFrame::Expr { braces };
                        } else {
                            sp -= 1;
                        }
                    }
                    b'"' if src.get(*p + 1) == Some(&b'"') && src.get(*p + 2) == Some(&b'"') => {
                        let open = Span::new(*line, *col);
                        *p += 3;
                        *col += 3;
                        if src.get(*p) == Some(&b'\n') {
                            *p += 1;
                            *line += 1;
                            *col = 1;
                        } else if src.get(*p) == Some(&b'\r') && src.get(*p + 1) == Some(&b'\n') {
                            *p += 2;
                            *line += 1;
                            *col = 1;
                        } else {
                            return Err(MultilineScanErr::MissingNewlineAfterOpen(open));
                        }
                        if sp >= stack.len() {
                            return Err(MultilineScanErr::NestingLimit(open));
                        }
                        stack[sp] = SkipFrame::MultiStr {
                            at_line_start: true,
                        };
                        sp += 1;
                    }
                    b'"' => {
                        let open = Span::new(*line, *col);
                        *p += 1;
                        *col += 1;
                        if sp >= stack.len() {
                            return Err(MultilineScanErr::NestingLimit(open));
                        }
                        stack[sp] = SkipFrame::SingleStr;
                        sp += 1;
                    }
                    b'\n' => {
                        *p += 1;
                        *line += 1;
                        *col = 1;
                    }
                    _ => {
                        *p += 1;
                        *col += 1;
                    }
                }
            }
            SkipFrame::SingleStr => {
                let b = src[*p];
                match b {
                    b'"' => {
                        *p += 1;
                        *col += 1;
                        sp -= 1;
                    }
                    b'\n' | b'\r' => {
                        return Err(MultilineScanErr::RawNewlineInInnerString(Span::new(
                            *line, *col,
                        )));
                    }
                    b'\\' => {
                        *p += 1;
                        *col += 1;
                        if *p < src.len() && src[*p] != b'\n' && src[*p] != b'\r' {
                            *p += 1;
                            *col += 1;
                        }
                    }
                    b'$' if src.get(*p + 1) == Some(&b'{') => {
                        let span = Span::new(*line, *col);
                        *p += 2;
                        *col += 2;
                        if sp >= stack.len() {
                            return Err(MultilineScanErr::NestingLimit(span));
                        }
                        stack[sp] = SkipFrame::Expr { braces: 0 };
                        sp += 1;
                    }
                    _ => {
                        *p += 1;
                        *col += 1;
                    }
                }
            }
            SkipFrame::MultiStr { at_line_start } => {
                let b = src[*p];
                if at_line_start && (b == b' ' || b == b'\t') {
                    *p += 1;
                    *col += 1;
                    continue;
                }
                if b == b'"' && src.get(*p + 1) == Some(&b'"') && src.get(*p + 2) == Some(&b'"') {
                    if at_line_start {
                        *p += 3;
                        *col += 3;
                        sp -= 1;
                        continue;
                    }
                    return Err(MultilineScanErr::MisplacedClosing(Span::new(*line, *col)));
                }
                if b == b'\n' {
                    *p += 1;
                    *line += 1;
                    *col = 1;
                    stack[sp - 1] = SkipFrame::MultiStr {
                        at_line_start: true,
                    };
                    continue;
                }
                if b == b'\r' {
                    if src.get(*p + 1) == Some(&b'\n') {
                        *p += 2;
                        *line += 1;
                        *col = 1;
                        stack[sp - 1] = SkipFrame::MultiStr {
                            at_line_start: true,
                        };
                        continue;
                    }
                    return Err(MultilineScanErr::LoneCr(Span::new(*line, *col)));
                }
                stack[sp - 1] = SkipFrame::MultiStr {
                    at_line_start: false,
                };
                if b == b'\\' {
                    *p += 1;
                    *col += 1;
                    if *p < src.len() && src[*p] != b'\n' && src[*p] != b'\r' {
                        *p += 1;
                        *col += 1;
                    }
                } else if b == b'$' && src.get(*p + 1) == Some(&b'{') {
                    let span = Span::new(*line, *col);
                    *p += 2;
                    *col += 2;
                    if sp >= stack.len() {
                        return Err(MultilineScanErr::NestingLimit(span));
                    }
                    stack[sp] = SkipFrame::Expr { braces: 0 };
                    sp += 1;
                } else {
                    *p += 1;
                    *col += 1;
                }
            }
        }
    }

    if sp > 0 {
        return Err(MultilineScanErr::Unterminated(outer_open));
    }
    Ok(())
}
