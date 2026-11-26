//! Rin ソース文字列を位置付きの字句トークン列へ分解する。

#[cfg(feature = "std-value")]
use crate::error::Error;
use crate::error::Span;

mod work;

pub(crate) use work::{WorkLexer, WorkToken, WorkTokenKind};

#[cfg(feature = "std-value")]
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
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
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    Char(char),
    Eof,
}

#[cfg(feature = "std-value")]
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// 直前に空白やコメントがない場合に true となる。
    pub attached: bool,
}

#[cfg(feature = "std-value")]
pub fn tokenize(input: &str) -> Result<Vec<Token>, Error> {
    let mut tokens = Vec::new();
    tokenize_into(input, &mut tokens)?;
    Ok(tokens)
}

#[cfg(feature = "std-value")]
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
#[cfg(feature = "std-value")]
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

    /// `start` バイト位置から始まる 1 Unicode スカラー値を O(1) で返す。
    fn char_at(&self, start: usize) -> char {
        let slice = &self.src[start..(start + 4).min(self.src.len())];
        let valid = match std::str::from_utf8(slice) {
            Ok(s) => s,
            Err(e) => std::str::from_utf8(&slice[..e.valid_up_to()]).unwrap_or(""),
        };
        valid.chars().next().unwrap_or(char::REPLACEMENT_CHARACTER)
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
}

#[cfg(feature = "std-value")]
impl<'a> Lexer<'a> {
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
            b':' => {
                self.bump();
                TokenKind::Colon
            }
            b',' => {
                self.bump();
                if self.peek() == Some(b'@') {
                    self.bump();
                    TokenKind::UnquoteSplicing
                } else {
                    TokenKind::Unquote
                }
            }
            b'`' => {
                self.bump();
                TokenKind::Quasiquote
            }
            b'"' => TokenKind::Str(self.lex_string(span)?),
            b'\'' => self.lex_quote_or_char(span)?,
            b'0'..=b'9' => self.lex_number(span, false)?,
            b'-' if self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) => {
                self.bump();
                self.lex_number(span, true)?
            }
            _ if is_ident_start(b) => TokenKind::Ident(self.lex_ident(span)?),
            _ => {
                let ch = self.char_at(self.pos);
                return Err(Error::lex(
                    "src.token",
                    span,
                    format!("unexpected character {ch:?}"),
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
        let esc_pos = self.pos;
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
            b'u' => self.lex_unicode_escape(span),
            _ => {
                let ch = self.char_at(esc_pos);
                for _ in 1..ch.len_utf8() {
                    self.bump();
                }
                Err(Error::lex(
                    "src.escape",
                    span,
                    format!("unknown escape sequence '\\{ch}'"),
                ))
            }
        }
    }

    fn lex_unicode_escape(&mut self, span: Span) -> Result<char, Error> {
        if self.bump() != Some(b'{') {
            return Err(Error::lex(
                "src.escape",
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
                        return Err(Error::lex(
                            "src.escape",
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
                    return Err(Error::lex(
                        "src.escape",
                        span,
                        "invalid unicode escape sequence",
                    ));
                }
            }
        }
        if digits == 0 {
            return Err(Error::lex(
                "src.escape",
                span,
                "empty unicode escape sequence",
            ));
        }
        char::from_u32(val).ok_or_else(|| {
            Error::lex(
                "src.escape",
                span,
                format!("invalid unicode scalar value U+{val:X}"),
            )
        })
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

    fn lex_quote_or_char(&mut self, span: Span) -> Result<TokenKind, Error> {
        let saved = (self.pos, self.line, self.col);
        self.bump(); // 開始の '\''
        let Some(b) = self.bump() else {
            return Ok(TokenKind::Quote);
        };
        if matches!(b, b'\'' | b'\n' | b'\r') {
            (self.pos, self.line, self.col) = saved;
            self.bump();
            return Ok(TokenKind::Quote);
        }
        let ch = if b == b'\\' {
            match self.lex_escape(span) {
                Ok(c) => c,
                Err(e) => {
                    if self.peek() == Some(b'\'') {
                        return Err(e);
                    }
                    (self.pos, self.line, self.col) = saved;
                    self.bump();
                    return Ok(TokenKind::Quote);
                }
            }
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
        if self.peek() != Some(b'\'') {
            (self.pos, self.line, self.col) = saved;
            self.bump();
            return Ok(TokenKind::Quote);
        }
        self.bump(); // 閉じの '\''
        if let Some(after) = self.peek()
            && !is_char_delimiter(after)
        {
            if is_quote_rewind_char(b) {
                (self.pos, self.line, self.col) = saved;
                self.bump();
                return Ok(TokenKind::Quote);
            }
            return Err(Error::lex(
                "src.char",
                span,
                "unexpected character immediately after char literal",
            ));
        }
        Ok(TokenKind::Char(ch))
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

    fn lex_ident(&mut self, span: Span) -> Result<String, Error> {
        let mut out = String::new();
        while let Some(b) = self.peek() {
            if is_ident_continue(b) {
                out.push(b as char);
                self.bump();
            } else {
                break;
            }
        }
        if self.peek() == Some(b'\'') {
            return Err(Error::lex(
                "src.ident",
                span,
                "single quote '\\'' is not allowed in identifier",
            ));
        }
        Ok(out)
    }
}

pub(crate) fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic()
        || matches!(
            b,
            b'_' | b'+' | b'-' | b'*' | b'/' | b'%' | b'=' | b'!' | b'<' | b'>'
        )
}

pub(crate) fn is_ident_continue(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

pub(crate) fn is_valid_ident(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    is_ident_start(first) && bytes[1..].iter().copied().all(is_ident_continue)
}

pub(crate) fn is_char_delimiter(b: u8) -> bool {
    b.is_ascii_whitespace() || matches!(b, b';' | b')' | b']' | b'}')
}

pub(crate) fn is_quote_rewind_char(b: u8) -> bool {
    matches!(b, b'`' | b',' | b'(' | b'[' | b'{')
}
