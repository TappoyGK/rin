//! 作業領域 (`WorkArena`) 上で動作するゼロヒープ・ストリーミング字句解析器。

use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::mem::{AllocCore, RawStr, RawVec, WorkArena};

use super::{Lexer, is_ident_continue, is_ident_start, named_char, unicode};

/// 作業領域 (`WorkArena`) 上に構築する字句トークン種別（`Copy`・システムヒープ不使用）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WorkTokenKind {
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Dot,
    Colon,
    Quote,
    Quasiquote,
    Unquote,
    UnquoteSplicing,
    Ident(RawStr),
    Int(i64),
    Float(f64),
    Str(RawStr),
    Char(char),
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WorkToken {
    pub kind: WorkTokenKind,
    pub span: Span,
    pub attached: bool,
}

/// 作業領域 `WorkArena` 向けのストリーミング字句解析器（トークン配列を確保せず 1 トークンずつ返す）。
pub(crate) struct WorkLexer<'a> {
    inner: Lexer<'a>,
}

impl<'a> WorkLexer<'a> {
    pub(crate) fn new(input: &'a str) -> Self {
        Self {
            inner: Lexer::new(input),
        }
    }

    pub(crate) fn next_token(
        &mut self,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
    ) -> Result<WorkToken, ErrRecorded> {
        self.inner.next_work_token(work, core)
    }
}

impl Lexer<'_> {
    fn next_work_token(
        &mut self,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
    ) -> Result<WorkToken, ErrRecorded> {
        let skipped = self.skip_ws_and_comments();
        let attached = !skipped && self.pos > 0;
        let span = self.span();

        let Some(b) = self.peek() else {
            return Ok(WorkToken {
                kind: WorkTokenKind::Eof,
                span,
                attached: false,
            });
        };

        let kind = match b {
            b'(' => {
                self.bump();
                WorkTokenKind::LParen
            }
            b')' => {
                self.bump();
                WorkTokenKind::RParen
            }
            b'[' => {
                self.bump();
                WorkTokenKind::LBracket
            }
            b']' => {
                self.bump();
                WorkTokenKind::RBracket
            }
            b'{' => {
                self.bump();
                WorkTokenKind::LBrace
            }
            b'}' => {
                self.bump();
                WorkTokenKind::RBrace
            }
            b'.' => {
                self.bump();
                WorkTokenKind::Dot
            }
            b':' => {
                self.bump();
                WorkTokenKind::Colon
            }
            b',' => {
                self.bump();
                if self.peek() == Some(b'@') {
                    self.bump();
                    WorkTokenKind::UnquoteSplicing
                } else {
                    WorkTokenKind::Unquote
                }
            }
            b'`' => {
                self.bump();
                WorkTokenKind::Quasiquote
            }
            b'"' => WorkTokenKind::Str(self.lex_work_string(span, work, core)?),
            b'\'' => {
                self.bump();
                WorkTokenKind::Quote
            }
            b'\\' => self.lex_work_char(span, core)?,
            b'0'..=b'9' => self.lex_work_number(span, false, work, core)?,
            b'-' if self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) => {
                self.bump();
                self.lex_work_number(span, true, work, core)?
            }
            _ if is_ident_start(b) => WorkTokenKind::Ident(self.lex_work_ident(span, work, core)?),
            _ => {
                let ch = self.char_at(self.pos);
                return Err(core.err_fmt(
                    "src.token",
                    ErrorKind::Lex,
                    span,
                    format_args!("unexpected character {ch:?}"),
                ));
            }
        };

        Ok(WorkToken {
            kind,
            span,
            attached,
        })
    }

    fn lex_work_escape(
        &mut self,
        span: Span,
        core: &mut AllocCore<'_>,
    ) -> Result<char, ErrRecorded> {
        let esc_pos = self.pos;
        let Some(esc) = self.bump() else {
            return Err(core.err_str(
                "src.escape",
                ErrorKind::Lex,
                span,
                "unterminated escape sequence",
            ));
        };
        match esc {
            b'n' => Ok('\n'),
            b'r' => Ok('\r'),
            b't' => Ok('\t'),
            b'\\' => Ok('\\'),
            b'"' => Ok('"'),
            b'0' => Ok('\0'),
            b'u' => self.lex_work_unicode_escape(span, core),
            _ => {
                let ch = self.char_at(esc_pos);
                for _ in 1..ch.len_utf8() {
                    self.bump();
                }
                Err(core.err_fmt(
                    "src.escape",
                    ErrorKind::Lex,
                    span,
                    format_args!("unknown escape sequence '\\{ch}'"),
                ))
            }
        }
    }

    fn lex_work_unicode_escape(
        &mut self,
        span: Span,
        core: &mut AllocCore<'_>,
    ) -> Result<char, ErrRecorded> {
        if self.bump() != Some(b'{') {
            return Err(core.err_str(
                "src.escape",
                ErrorKind::Lex,
                span,
                "expected '{' in unicode escape sequence",
            ));
        }
        let mut val: u32 = 0;
        let mut digits = 0usize;
        loop {
            match self.bump() {
                Some(b'}') => break,
                Some(b) if b.is_ascii_hexdigit() => {
                    digits += 1;
                    if digits > 6 {
                        return Err(core.err_str(
                            "src.escape",
                            ErrorKind::Lex,
                            span,
                            "unicode escape sequence has too many hex digits",
                        ));
                    }
                    let d = match b {
                        b'0'..=b'9' => (b - b'0') as u32,
                        b'a'..=b'f' => (b - b'a' + 10) as u32,
                        b'A'..=b'F' => (b - b'A' + 10) as u32,
                        _ => unreachable!(),
                    };
                    val = (val << 4) | d;
                }
                _ => {
                    return Err(core.err_str(
                        "src.escape",
                        ErrorKind::Lex,
                        span,
                        "invalid unicode escape sequence",
                    ));
                }
            }
        }
        if digits == 0 {
            return Err(core.err_str(
                "src.escape",
                ErrorKind::Lex,
                span,
                "empty unicode escape sequence",
            ));
        }
        char::from_u32(val).ok_or_else(|| {
            core.err_fmt(
                "src.escape",
                ErrorKind::Lex,
                span,
                format_args!("invalid unicode scalar value U+{val:X}"),
            )
        })
    }

    fn lex_work_char(
        &mut self,
        span: Span,
        core: &mut AllocCore<'_>,
    ) -> Result<WorkTokenKind, ErrRecorded> {
        self.bump(); // 開始の '\\'
        let start = self.pos;
        let Some(first) = self.bump_char() else {
            return Err(core.err_str(
                "src.char",
                ErrorKind::Lex,
                span,
                "unterminated char literal",
            ));
        };
        if unicode::is_char_forbidden_raw(first) {
            return Err(core.err_str(
                "src.char",
                ErrorKind::Lex,
                span,
                "invisible or control character is not allowed directly after '\\' in char literal",
            ));
        }
        if first == 'u' && self.peek() == Some(b'{') {
            return self
                .lex_work_unicode_escape(span, core)
                .map(WorkTokenKind::Char);
        }
        // 後続が入力末尾または ASCII 非名前文字なら 1 文字で確定（非 ASCII の二分探索を省略）。
        let Some(next_b) = self.peek() else {
            return Ok(WorkTokenKind::Char(first));
        };
        if next_b < 0x80 && !(next_b.is_ascii_alphanumeric() || next_b == b'_') {
            return Ok(WorkTokenKind::Char(first));
        }
        if unicode::is_char_name_char(first) {
            let mut count = 1usize;
            while let Some(nb) = self.peek() {
                if nb < 0x80 {
                    if nb.is_ascii_alphanumeric() || nb == b'_' {
                        self.bump();
                        count += 1;
                    } else {
                        break;
                    }
                } else {
                    let next_ch = self.char_at(self.pos);
                    if !unicode::is_char_name_char(next_ch) {
                        break;
                    }
                    self.bump_char();
                    count += 1;
                }
            }
            if count == 1 {
                return Ok(WorkTokenKind::Char(first));
            }
            let name = &self.src[start..self.pos];
            if let Some(ch) = named_char(name) {
                return Ok(WorkTokenKind::Char(ch));
            }
            let name_str = std::str::from_utf8(name).unwrap_or("");
            return Err(core.err_fmt(
                "src.char",
                ErrorKind::Lex,
                span,
                format_args!("unknown character name '\\{name_str}'"),
            ));
        }
        Ok(WorkTokenKind::Char(first))
    }

    fn lex_work_ident(
        &mut self,
        span: Span,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
    ) -> Result<RawStr, ErrRecorded> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if is_ident_continue(b) {
                self.bump();
            } else {
                break;
            }
        }
        if self.peek() == Some(b'\'') {
            return Err(core.err_str(
                "src.ident",
                ErrorKind::Lex,
                span,
                "single quote '\\'' is not allowed in identifier",
            ));
        }
        // SAFETY: `is_ident_continue` は ASCII のみなので有効な UTF-8。
        let s = unsafe { std::str::from_utf8_unchecked(&self.src[start..self.pos]) };
        match RawStr::from_work(work, core, s) {
            Some(r) => Ok(r),
            None => Err(core.err_oom(span)),
        }
    }

    fn lex_work_string(
        &mut self,
        span: Span,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
    ) -> Result<RawStr, ErrRecorded> {
        self.bump(); // 開始の '"'
        let mut buf = RawVec::<u8>::new();
        loop {
            let Some(b) = self.bump() else {
                return Err(core.err_str(
                    "src.string",
                    ErrorKind::Lex,
                    span,
                    "unterminated string literal",
                ));
            };
            match b {
                b'"' => {
                    if buf.is_empty() {
                        return Ok(RawStr::empty());
                    }
                    let slice = buf.as_slice();
                    // SAFETY: すべてのバイトは有効な UTF-8 として追加済み。
                    let ptr = std::ptr::NonNull::new(slice.as_ptr() as *mut u8).unwrap();
                    return Ok(unsafe { RawStr::from_raw_utf8(ptr, slice.len()) });
                }
                b'\\' => {
                    let ch = self.lex_work_escape(span, core)?;
                    let mut enc = [0u8; 4];
                    let bytes = ch.encode_utf8(&mut enc).as_bytes();
                    if buf.extend_from_slice_work(work, core, bytes).is_none() {
                        return Err(core.err_oom(span));
                    }
                }
                _ if b < 0x80 => {
                    if buf.push_work(work, core, b).is_none() {
                        return Err(core.err_oom(span));
                    }
                }
                _ => {
                    let start = self.pos - 1;
                    while let Some(next) = self.peek() {
                        if (next & 0xC0) != 0x80 {
                            break;
                        }
                        self.bump();
                    }
                    let Ok(s) = std::str::from_utf8(&self.src[start..self.pos]) else {
                        return Err(core.err_str(
                            "src.encoding",
                            ErrorKind::Lex,
                            span,
                            "invalid UTF-8 in string literal",
                        ));
                    };
                    if buf
                        .extend_from_slice_work(work, core, s.as_bytes())
                        .is_none()
                    {
                        return Err(core.err_oom(span));
                    }
                }
            }
        }
    }

    fn lex_work_number(
        &mut self,
        span: Span,
        neg: bool,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
    ) -> Result<WorkTokenKind, ErrRecorded> {
        let mut stack_buf = [0u8; 128];
        let mut stack_len = 0usize;
        let mut heap_buf = RawVec::<u8>::new();
        let mut spilled = false;

        let push_byte = |b: u8,
                         stack_buf: &mut [u8; 128],
                         stack_len: &mut usize,
                         heap_buf: &mut RawVec<u8>,
                         spilled: &mut bool,
                         work: &mut WorkArena,
                         core: &mut AllocCore<'_>|
         -> Result<(), ErrRecorded> {
            if !*spilled {
                if *stack_len < stack_buf.len() {
                    stack_buf[*stack_len] = b;
                    *stack_len += 1;
                    return Ok(());
                }
                if heap_buf
                    .extend_from_slice_work(work, core, &stack_buf[..*stack_len])
                    .is_none()
                {
                    return Err(core.err_oom(span));
                }
                *spilled = true;
            }
            if heap_buf.push_work(work, core, b).is_none() {
                return Err(core.err_oom(span));
            }
            Ok(())
        };

        if !neg && self.peek() == Some(b'0') && matches!(self.peek_at(1), Some(b'x') | Some(b'X')) {
            self.bump();
            self.bump();
            while let Some(b) = self.peek() {
                if b.is_ascii_hexdigit() {
                    push_byte(
                        b,
                        &mut stack_buf,
                        &mut stack_len,
                        &mut heap_buf,
                        &mut spilled,
                        work,
                        core,
                    )?;
                    self.bump();
                } else if b == b'_' {
                    self.bump();
                } else {
                    break;
                }
            }
            let bytes = if spilled {
                heap_buf.as_slice()
            } else {
                &stack_buf[..stack_len]
            };
            if bytes.is_empty() {
                return Err(core.err_str(
                    "src.int",
                    ErrorKind::Lex,
                    span,
                    "invalid hexadecimal literal",
                ));
            }
            // SAFETY: ASCII hex digits only.
            let hex = unsafe { std::str::from_utf8_unchecked(bytes) };
            let Ok(n) = i64::from_str_radix(hex, 16) else {
                return Err(core.err_str(
                    "src.int",
                    ErrorKind::Lex,
                    span,
                    "integer literal out of i64 range",
                ));
            };
            return Ok(WorkTokenKind::Int(n));
        }

        if neg {
            push_byte(
                b'-',
                &mut stack_buf,
                &mut stack_len,
                &mut heap_buf,
                &mut spilled,
                work,
                core,
            )?;
        }
        let mut is_float = false;

        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                push_byte(
                    b,
                    &mut stack_buf,
                    &mut stack_len,
                    &mut heap_buf,
                    &mut spilled,
                    work,
                    core,
                )?;
                self.bump();
            } else if b == b'_' {
                self.bump();
            } else {
                break;
            }
        }

        if self.peek() == Some(b'.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            push_byte(
                b'.',
                &mut stack_buf,
                &mut stack_len,
                &mut heap_buf,
                &mut spilled,
                work,
                core,
            )?;
            self.bump();
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    push_byte(
                        b,
                        &mut stack_buf,
                        &mut stack_len,
                        &mut heap_buf,
                        &mut spilled,
                        work,
                        core,
                    )?;
                    self.bump();
                } else if b == b'_' {
                    self.bump();
                } else {
                    break;
                }
            }
        }

        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            push_byte(
                b'e',
                &mut stack_buf,
                &mut stack_len,
                &mut heap_buf,
                &mut spilled,
                work,
                core,
            )?;
            self.bump();
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                let sign = self.bump().unwrap();
                push_byte(
                    sign,
                    &mut stack_buf,
                    &mut stack_len,
                    &mut heap_buf,
                    &mut spilled,
                    work,
                    core,
                )?;
            }
            let before = if spilled { heap_buf.len() } else { stack_len };
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    push_byte(
                        b,
                        &mut stack_buf,
                        &mut stack_len,
                        &mut heap_buf,
                        &mut spilled,
                        work,
                        core,
                    )?;
                    self.bump();
                } else if b == b'_' {
                    self.bump();
                } else {
                    break;
                }
            }
            let after = if spilled { heap_buf.len() } else { stack_len };
            if after == before {
                return Err(core.err_str(
                    "src.float",
                    ErrorKind::Lex,
                    span,
                    "invalid float exponent",
                ));
            }
        }

        let bytes = if spilled {
            heap_buf.as_slice()
        } else {
            &stack_buf[..stack_len]
        };
        // SAFETY: ASCII number chars only.
        let s = unsafe { std::str::from_utf8_unchecked(bytes) };
        if is_float {
            let Ok(x) = s.parse::<f64>() else {
                return Err(core.err_str(
                    "src.float",
                    ErrorKind::Lex,
                    span,
                    "invalid float literal",
                ));
            };
            Ok(WorkTokenKind::Float(x))
        } else {
            let Ok(n) = s.parse::<i64>() else {
                return Err(core.err_str(
                    "src.int",
                    ErrorKind::Lex,
                    span,
                    "integer literal out of i64 range",
                ));
            };
            Ok(WorkTokenKind::Int(n))
        }
    }
}
