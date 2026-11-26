//! Rin ソース文字列を位置付きの字句トークン列へ分解する。

use crate::error::{Error, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Dot,
    Comma,
    Colon,
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    Char(char),
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// 直前に空白やコメントがない場合に true となる。
    pub attached: bool,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, Error> {
    let mut tokens = Vec::new();
    tokenize_into(input, &mut tokens)?;
    Ok(tokens)
}

pub(crate) fn tokenize_into(input: &str, tokens: &mut Vec<Token>) -> Result<(), Error> {
    tokens.clear();
    let mut lexer = Lexer::new(input);
    loop {
        let tok = lexer.next_token()?;
        let is_eof = tok.kind == TokenKind::Eof;
        tokens.push(tok);
        if is_eof {
            break;
        }
    }
    Ok(())
}

/// 整形器向け。通常のTokenを大きくせず、原文の範囲を別に返す。
pub(crate) fn tokenize_lossless(
    input: &str,
) -> Result<Vec<(Token, std::ops::Range<usize>)>, Error> {
    let mut lines = vec![0];
    lines.extend(input.match_indices('\n').map(|(i, _)| i + 1));
    let mut lexer = Lexer::new(input);
    let mut result = Vec::new();
    loop {
        let token = lexer.next_token()?;
        if token.kind == TokenKind::Eof {
            break;
        }
        let start = lines[token.span.line - 1] + token.span.col - 1;
        result.push((token, start..lexer.pos));
    }
    Ok(result)
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            src: input.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn span(&self) -> Span {
        Span::new(self.line, self.col)
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.src.get(self.pos + offset).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.src.get(self.pos).copied()?;
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(b)
    }

    fn skip_ws_and_comments(&mut self) -> bool {
        let mut skipped = false;
        loop {
            while let Some(b) = self.peek() {
                if b.is_ascii_whitespace() {
                    skipped = true;
                    self.bump();
                } else {
                    break;
                }
            }
            if self.peek() == Some(b';') {
                skipped = true;
                while let Some(b) = self.bump() {
                    if b == b'\n' {
                        break;
                    }
                }
                continue;
            }
            break;
        }
        skipped
    }

    fn next_token(&mut self) -> Result<Token, Error> {
        let skipped = self.skip_ws_and_comments();
        let attached = !skipped && self.pos > 0;
        let span = self.span();

        let Some(b) = self.peek() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                span,
                attached: false,
            });
        };

        let kind = match b {
            b'(' => {
                self.bump();
                TokenKind::LParen
            }
            b')' => {
                self.bump();
                TokenKind::RParen
            }
            b'[' => {
                self.bump();
                TokenKind::LBracket
            }
            b']' => {
                self.bump();
                TokenKind::RBracket
            }
            b'{' => {
                self.bump();
                TokenKind::LBrace
            }
            b'}' => {
                self.bump();
                TokenKind::RBrace
            }
            b'.' => {
                self.bump();
                TokenKind::Dot
            }
            b',' => {
                self.bump();
                TokenKind::Comma
            }
            b':' => {
                self.bump();
                TokenKind::Colon
            }
            b'"' => TokenKind::Str(self.lex_string(span)?),
            b'\'' => TokenKind::Char(self.lex_char(span)?),
            b'0'..=b'9' => self.lex_number(span, false)?,
            b'-' if self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) => {
                self.bump();
                self.lex_number(span, true)?
            }
            _ if is_ident_start(b) => TokenKind::Ident(self.lex_ident()),
            _ => {
                return Err(Error::lex(
                    "src.token",
                    span,
                    format!("unexpected character {:?}", b as char),
                ));
            }
        };

        Ok(Token {
            kind,
            span,
            attached,
        })
    }

    fn lex_escape(&mut self, span: Span) -> Result<char, Error> {
        let Some(esc) = self.bump() else {
            return Err(Error::lex(
                "src.escape",
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
            b'\'' => Ok('\''),
            b'0' => Ok('\0'),
            other => Err(Error::lex(
                "src.escape",
                span,
                format!("unknown escape sequence \\{}", other as char),
            )),
        }
    }

    fn lex_string(&mut self, span: Span) -> Result<String, Error> {
        self.bump(); // 開始の '"'
        let mut out = String::new();
        loop {
            let Some(b) = self.bump() else {
                return Err(Error::lex(
                    "src.string",
                    span,
                    "unterminated string literal",
                ));
            };
            match b {
                b'"' => return Ok(out),
                b'\\' => out.push(self.lex_escape(span)?),
                _ if b < 0x80 => out.push(b as char),
                _ => {
                    // スライスから UTF-8 マルチバイト文字を読み取る。
                    let start = self.pos - 1;
                    while let Some(next) = self.peek() {
                        if (next & 0xC0) != 0x80 {
                            break;
                        }
                        self.bump();
                    }
                    let s = std::str::from_utf8(&self.src[start..self.pos]).map_err(|_| {
                        Error::lex("src.encoding", span, "invalid UTF-8 in string literal")
                    })?;
                    out.push_str(s);
                }
            }
        }
    }

    fn lex_char(&mut self, span: Span) -> Result<char, Error> {
        self.bump(); // 開始の '\''
        let Some(b) = self.bump() else {
            return Err(Error::lex("src.char", span, "unterminated char literal"));
        };
        let ch = if b == b'\\' {
            self.lex_escape(span)?
        } else if b < 0x80 {
            b as char
        } else {
            let start = self.pos - 1;
            while let Some(next) = self.peek() {
                if (next & 0xC0) != 0x80 {
                    break;
                }
                self.bump();
            }
            let s = std::str::from_utf8(&self.src[start..self.pos])
                .map_err(|_| Error::lex("src.encoding", span, "invalid UTF-8 in char literal"))?;
            s.chars()
                .next()
                .ok_or_else(|| Error::lex("src.char", span, "empty char literal"))?
        };
        if self.bump() != Some(b'\'') {
            return Err(Error::lex("src.char", span, "unterminated char literal"));
        }
        Ok(ch)
    }

    fn lex_number(&mut self, span: Span, neg: bool) -> Result<TokenKind, Error> {
        if !neg && self.peek() == Some(b'0') && matches!(self.peek_at(1), Some(b'x') | Some(b'X')) {
            self.bump();
            self.bump();
            let mut hex = String::new();
            while let Some(b) = self.peek() {
                if b.is_ascii_hexdigit() {
                    hex.push(b as char);
                    self.bump();
                } else if b == b'_' {
                    self.bump();
                } else {
                    break;
                }
            }
            if hex.is_empty() {
                return Err(Error::lex("src.int", span, "invalid hexadecimal literal"));
            }
            let n = i64::from_str_radix(&hex, 16)
                .map_err(|_| Error::lex("src.int", span, "integer literal out of i64 range"))?;
            return Ok(TokenKind::Int(n));
        }

        let mut buf = String::new();
        if neg {
            buf.push('-');
        }
        let mut is_float = false;

        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                buf.push(b as char);
                self.bump();
            } else if b == b'_' {
                self.bump();
            } else {
                break;
            }
        }

        if self.peek() == Some(b'.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            buf.push('.');
            self.bump();
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    buf.push(b as char);
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
            buf.push('e');
            self.bump();
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                buf.push(self.bump().unwrap() as char);
            }
            let before = buf.len();
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    buf.push(b as char);
                    self.bump();
                } else if b == b'_' {
                    self.bump();
                } else {
                    break;
                }
            }
            if buf.len() == before {
                return Err(Error::lex("src.float", span, "invalid float exponent"));
            }
        }

        if is_float {
            let x: f64 = buf
                .parse()
                .map_err(|_| Error::lex("src.float", span, "invalid float literal"))?;
            Ok(TokenKind::Float(x))
        } else {
            let n: i64 = buf
                .parse()
                .map_err(|_| Error::lex("src.int", span, "integer literal out of i64 range"))?;
            Ok(TokenKind::Int(n))
        }
    }

    fn lex_ident(&mut self) -> String {
        let mut out = String::new();
        while let Some(b) = self.peek() {
            if is_ident_continue(b) {
                out.push(b as char);
                self.bump();
            } else {
                break;
            }
        }
        out
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic()
        || matches!(
            b,
            b'_' | b'+' | b'-' | b'*' | b'/' | b'%' | b'=' | b'!' | b'<' | b'>'
        )
}

fn is_ident_continue(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

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
    Comma,
    Colon,
    Ident(crate::mem::RawStr),
    Int(i64),
    Float(f64),
    Str(crate::mem::RawStr),
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
        work: &mut crate::mem::WorkArena,
        core: &mut crate::mem::AllocCore<'_>,
    ) -> Result<WorkToken, Error> {
        self.inner.next_work_token(work, core)
    }
}

impl Lexer<'_> {
    fn next_work_token(
        &mut self,
        work: &mut crate::mem::WorkArena,
        core: &mut crate::mem::AllocCore<'_>,
    ) -> Result<WorkToken, Error> {
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
            b',' => {
                self.bump();
                WorkTokenKind::Comma
            }
            b':' => {
                self.bump();
                WorkTokenKind::Colon
            }
            b'"' => WorkTokenKind::Str(self.lex_work_string(span, work, core)?),
            b'\'' => WorkTokenKind::Char(self.lex_char(span)?),
            b'0'..=b'9' => self.lex_work_number(span, false, work, core)?,
            b'-' if self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) => {
                self.bump();
                self.lex_work_number(span, true, work, core)?
            }
            _ if is_ident_start(b) => WorkTokenKind::Ident(self.lex_work_ident(span, work, core)?),
            _ => {
                return Err(Error::lex(
                    "src.token",
                    span,
                    format!("unexpected character {:?}", b as char),
                ));
            }
        };

        Ok(WorkToken {
            kind,
            span,
            attached,
        })
    }

    fn lex_work_ident(
        &mut self,
        span: Span,
        work: &mut crate::mem::WorkArena,
        core: &mut crate::mem::AllocCore<'_>,
    ) -> Result<crate::mem::RawStr, Error> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if is_ident_continue(b) {
                self.bump();
            } else {
                break;
            }
        }
        // SAFETY: `is_ident_continue` は ASCII のみなので有効な UTF-8。
        let s = unsafe { std::str::from_utf8_unchecked(&self.src[start..self.pos]) };
        crate::mem::RawStr::from_work(work, core, s).ok_or_else(|| {
            Error::new(
                "eval.limits",
                crate::error::ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded while lexing identifier",
            )
        })
    }

    fn lex_work_string(
        &mut self,
        span: Span,
        work: &mut crate::mem::WorkArena,
        core: &mut crate::mem::AllocCore<'_>,
    ) -> Result<crate::mem::RawStr, Error> {
        self.bump(); // 開始の '"'
        let mut buf = crate::mem::RawVec::<u8>::new();
        let oom = || {
            Error::new(
                "eval.limits",
                crate::error::ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded while lexing string",
            )
        };
        loop {
            let Some(b) = self.bump() else {
                return Err(Error::lex(
                    "src.string",
                    span,
                    "unterminated string literal",
                ));
            };
            match b {
                b'"' => {
                    if buf.is_empty() {
                        return Ok(crate::mem::RawStr::empty());
                    }
                    let slice = buf.as_slice();
                    // SAFETY: すべてのバイトは有効な UTF-8 として追加済み。
                    let ptr = std::ptr::NonNull::new(slice.as_ptr() as *mut u8).unwrap();
                    return Ok(unsafe { crate::mem::RawStr::from_raw_utf8(ptr, slice.len()) });
                }
                b'\\' => {
                    let ch = self.lex_escape(span)?;
                    let mut enc = [0u8; 4];
                    let bytes = ch.encode_utf8(&mut enc).as_bytes();
                    buf.extend_from_slice_work(work, core, bytes)
                        .ok_or_else(oom)?;
                }
                _ if b < 0x80 => {
                    buf.push_work(work, core, b).ok_or_else(oom)?;
                }
                _ => {
                    let start = self.pos - 1;
                    while let Some(next) = self.peek() {
                        if (next & 0xC0) != 0x80 {
                            break;
                        }
                        self.bump();
                    }
                    let s = std::str::from_utf8(&self.src[start..self.pos]).map_err(|_| {
                        Error::lex("src.encoding", span, "invalid UTF-8 in string literal")
                    })?;
                    buf.extend_from_slice_work(work, core, s.as_bytes())
                        .ok_or_else(oom)?;
                }
            }
        }
    }

    fn lex_work_number(
        &mut self,
        span: Span,
        neg: bool,
        work: &mut crate::mem::WorkArena,
        core: &mut crate::mem::AllocCore<'_>,
    ) -> Result<WorkTokenKind, Error> {
        let mut stack_buf = [0u8; 128];
        let mut stack_len = 0usize;
        let mut heap_buf = crate::mem::RawVec::<u8>::new();
        let mut spilled = false;

        let push_byte = |b: u8,
                         stack_buf: &mut [u8; 128],
                         stack_len: &mut usize,
                         heap_buf: &mut crate::mem::RawVec<u8>,
                         spilled: &mut bool,
                         work: &mut crate::mem::WorkArena,
                         core: &mut crate::mem::AllocCore<'_>|
         -> Result<(), Error> {
            if !*spilled {
                if *stack_len < stack_buf.len() {
                    stack_buf[*stack_len] = b;
                    *stack_len += 1;
                    return Ok(());
                }
                heap_buf
                    .extend_from_slice_work(work, core, &stack_buf[..*stack_len])
                    .ok_or_else(|| {
                        Error::new(
                            "eval.limits",
                            crate::error::ErrorKind::TooMuchMemory,
                            span,
                            "memory limit exceeded while lexing number",
                        )
                    })?;
                *spilled = true;
            }
            heap_buf.push_work(work, core, b).ok_or_else(|| {
                Error::new(
                    "eval.limits",
                    crate::error::ErrorKind::TooMuchMemory,
                    span,
                    "memory limit exceeded while lexing number",
                )
            })
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
                return Err(Error::lex("src.int", span, "invalid hexadecimal literal"));
            }
            // SAFETY: ASCII hex digits only.
            let hex = unsafe { std::str::from_utf8_unchecked(bytes) };
            let n = i64::from_str_radix(hex, 16)
                .map_err(|_| Error::lex("src.int", span, "integer literal out of i64 range"))?;
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
                return Err(Error::lex("src.float", span, "invalid float exponent"));
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
            let x: f64 = s
                .parse()
                .map_err(|_| Error::lex("src.float", span, "invalid float literal"))?;
            Ok(WorkTokenKind::Float(x))
        } else {
            let n: i64 = s
                .parse()
                .map_err(|_| Error::lex("src.int", span, "integer literal out of i64 range"))?;
            Ok(WorkTokenKind::Int(n))
        }
    }
}
