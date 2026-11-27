//! Rin ソース文字列を位置付きの字句トークン列へ分解する。

use std::fmt::{self, Write as _};

#[cfg(feature = "std-value")]
use crate::error::Error;
use crate::error::Span;

mod unicode;
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
    TemplateStart(String),
    TemplateMid(String),
    TemplateEnd(String),
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
    let mut braces = Vec::new();
    loop {
        let tok = lexer.next_token(&mut braces)?;
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
    let mut braces = Vec::new();
    let mut result = Vec::new();
    loop {
        let token = lexer.next_token(&mut braces)?;
        if token.kind == TokenKind::Eof {
            break;
        }
        let start = lines[token.span.line - 1] + token.span.col - 1;
        if matches!(token.kind, TokenKind::TemplateStart(_)) {
            let mut depth = 1usize;
            while depth > 0 {
                let next = lexer.next_token(&mut braces)?;
                match next.kind {
                    TokenKind::TemplateStart(_) => depth += 1,
                    TokenKind::TemplateEnd(_) => depth -= 1,
                    TokenKind::Eof => break,
                    _ => {}
                }
            }
            let merged = Token {
                kind: TokenKind::Str(String::new()),
                span: token.span,
                attached: token.attached,
            };
            result.push((merged, start..lexer.pos));
        } else {
            result.push((token, start..lexer.pos));
        }
    }
    Ok(result)
}

const MAX_TEMPLATE_NESTING: usize = 256;

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
    work_braces: crate::mem::RawVec<u16>,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            src: input.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
            work_braces: crate::mem::RawVec::new(),
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

    fn bump_char(&mut self) -> Option<char> {
        let b = self.peek()?;
        if b < 0x80 {
            self.bump();
            Some(b as char)
        } else {
            let ch = self.char_at(self.pos);
            for _ in 0..ch.len_utf8() {
                self.bump();
            }
            Some(ch)
        }
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
    fn next_token(&mut self, braces: &mut Vec<u16>) -> Result<Token, Error> {
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
                if let Some(top) = braces.last_mut() {
                    *top = top.saturating_add(1);
                }
                TokenKind::LBrace
            }
            b'}' => {
                if let Some(top) = braces.last_mut() {
                    if *top > 0 {
                        *top -= 1;
                        self.bump();
                        TokenKind::RBrace
                    } else {
                        self.lex_string_segment(span, true, braces)?
                    }
                } else {
                    self.bump();
                    TokenKind::RBrace
                }
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
            b'"' => self.lex_string_segment(span, false, braces)?,
            b'\'' => {
                self.bump();
                TokenKind::Quote
            }
            b'\\' => self.lex_char(span)?,
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
            b'$' => Ok('$'),
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

    fn lex_string_segment(
        &mut self,
        span: Span,
        resuming: bool,
        braces: &mut Vec<u16>,
    ) -> Result<TokenKind, Error> {
        self.bump(); // 開始の '"' または再開の '}'
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
                b'"' => {
                    if resuming {
                        braces.pop();
                        return Ok(TokenKind::TemplateEnd(out));
                    }
                    return Ok(TokenKind::Str(out));
                }
                b'\n' | b'\r' => {
                    return Err(Error::lex(
                        "src.string",
                        span,
                        "raw newline is not allowed in string literal",
                    ));
                }
                b'$' if self.peek() == Some(b'{') => {
                    self.bump(); // '{'
                    if resuming {
                        return Ok(TokenKind::TemplateMid(out));
                    }
                    if braces.len() >= MAX_TEMPLATE_NESTING {
                        return Err(Error::parse(
                            "src.nesting-limit",
                            span,
                            "syntax nesting limit exceeded (256)",
                        ));
                    }
                    braces.push(0);
                    return Ok(TokenKind::TemplateStart(out));
                }
                b'\\' => out.push(self.lex_escape(span)?),
                _ if b < 0x80 => out.push(b as char),
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
                    out.push_str(s);
                }
            }
        }
    }

    fn lex_char(&mut self, span: Span) -> Result<TokenKind, Error> {
        self.bump(); // 開始の '\\'
        let start = self.pos;
        let Some(first) = self.bump_char() else {
            return Err(Error::lex("src.char", span, "unterminated char literal"));
        };
        if unicode::is_char_forbidden_raw(first) {
            return Err(Error::lex(
                "src.char",
                span,
                "invisible or control character is not allowed directly after '\\' in char literal",
            ));
        }
        if first == 'u' && self.peek() == Some(b'{') {
            return self.lex_unicode_escape(span).map(TokenKind::Char);
        }
        // 後続が入力末尾または ASCII 非名前文字なら 1 文字で確定（非 ASCII の二分探索を省略）。
        let Some(next_b) = self.peek() else {
            return Ok(TokenKind::Char(first));
        };
        if next_b < 0x80 && !(next_b.is_ascii_alphanumeric() || next_b == b'_') {
            return Ok(TokenKind::Char(first));
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
                return Ok(TokenKind::Char(first));
            }
            let name = &self.src[start..self.pos];
            if let Some(ch) = named_char(name) {
                return Ok(TokenKind::Char(ch));
            }
            let name_str = std::str::from_utf8(name).unwrap_or("");
            return Err(Error::lex(
                "src.char",
                span,
                format!("unknown character name '\\{name_str}'"),
            ));
        }
        Ok(TokenKind::Char(first))
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

pub(crate) fn named_char(name: &[u8]) -> Option<char> {
    match name {
        b"nul" => Some('\0'),
        b"tab" => Some('\t'),
        b"newline" => Some('\n'),
        b"return" => Some('\r'),
        b"space" => Some(' '),
        _ => None,
    }
}

/// `[value.display]` に従い `f64` 値を Suz 共通規則（最短往復・ゼロヒープ）で書き出す。
pub(crate) fn write_rin_float(f: &mut fmt::Formatter<'_>, x: f64) -> fmt::Result {
    if x.is_nan() {
        return f.write_str("nan");
    }
    if x.is_infinite() {
        return f.write_str(if x.is_sign_positive() { "inf" } else { "-inf" });
    }
    if x == 0.0 {
        return f.write_str(if x.is_sign_negative() { "-0.0" } else { "0.0" });
    }

    let abs = x.abs();
    if (1e-6..1e21).contains(&abs) {
        struct DotTracker<'a, 'b> {
            f: &'a mut fmt::Formatter<'b>,
            has_dot: bool,
        }
        impl fmt::Write for DotTracker<'_, '_> {
            #[inline(always)]
            fn write_str(&mut self, s: &str) -> fmt::Result {
                if !self.has_dot && s.as_bytes().contains(&b'.') {
                    self.has_dot = true;
                }
                self.f.write_str(s)
            }
        }
        let mut tracker = DotTracker { f, has_dot: false };
        write!(tracker, "{x}")?;
        if !tracker.has_dot {
            f.write_str(".0")?;
        }
        return Ok(());
    }

    struct StackBuf {
        buf: [u8; 48],
        len: usize,
    }
    impl StackBuf {
        #[inline]
        fn push_bytes(&mut self, b: &[u8]) {
            let avail = self.buf.len().saturating_sub(self.len);
            let n = b.len().min(avail);
            self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
            self.len += n;
        }
        #[inline]
        fn as_str(&self) -> &str {
            // SAFETY: ASCII バイトのみ書き込む。
            unsafe { std::str::from_utf8_unchecked(&self.buf[..self.len]) }
        }
    }
    impl fmt::Write for StackBuf {
        #[inline]
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.push_bytes(s.as_bytes());
            Ok(())
        }
    }

    let mut sb = StackBuf {
        buf: [0; 48],
        len: 0,
    };
    if x.is_sign_negative() {
        sb.push_bytes(b"-");
    }

    let _ = write!(&mut sb, "{abs:e}");
    let slice = &sb.buf[..sb.len];
    if let Some(e_pos) = slice.iter().position(|&b| b == b'e')
        && !slice[..e_pos].contains(&b'.')
    {
        // `1e21` -> `1.0e21`（指数表記でも小数点の両側に数字を置く）
        let mut out = StackBuf {
            buf: [0; 48],
            len: 0,
        };
        out.push_bytes(&slice[..e_pos]);
        out.push_bytes(b".0");
        out.push_bytes(&slice[e_pos..]);
        return f.write_str(out.as_str());
    }
    f.write_str(sb.as_str())
}

/// `[value.display]` に従い `char` 値を Rin 文字リテラル形式で書き出す。
pub(crate) fn write_rin_char(f: &mut fmt::Formatter<'_>, c: char) -> fmt::Result {
    match c {
        '\0' => f.write_str("\\nul"),
        '\t' => f.write_str("\\tab"),
        '\n' => f.write_str("\\newline"),
        '\r' => f.write_str("\\return"),
        ' ' => f.write_str("\\space"),
        c if c.is_ascii_control()
            || (!c.is_ascii() && unicode::is_char_display_escaped_non_ascii(c)) =>
        {
            write!(f, "\\u{{{:x}}}", c as u32)
        }
        c => {
            f.write_char('\\')?;
            f.write_char(c)
        }
    }
}

/// `[value.display]` に従い `string` 値を Rin 文字列リテラル形式で書き出す。
pub(crate) fn write_rin_str(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    f.write_char('"')?;
    let bytes = s.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            if (0x20..=0x7E).contains(&b) && b != b'\\' && b != b'"' && b != b'$' {
                i += 1;
                continue;
            }
            if start < i {
                f.write_str(&s[start..i])?;
            }
            match b {
                b'\0' => f.write_str("\\0")?,
                b'\n' => f.write_str("\\n")?,
                b'\r' => f.write_str("\\r")?,
                b'\t' => f.write_str("\\t")?,
                b'\\' => f.write_str("\\\\")?,
                b'"' => f.write_str("\\\"")?,
                b'$' => f.write_str("\\$")?,
                _ => write!(f, "\\u{{{:x}}}", b)?,
            }
            i += 1;
            start = i;
        } else {
            let ch = s[i..].chars().next().unwrap();
            let len = ch.len_utf8();
            if unicode::is_str_display_escaped_non_ascii(ch) {
                if start < i {
                    f.write_str(&s[start..i])?;
                }
                write!(f, "\\u{{{:x}}}", ch as u32)?;
                i += len;
                start = i;
            } else {
                i += len;
            }
        }
    }
    if start < bytes.len() {
        f.write_str(&s[start..])?;
    }
    f.write_char('"')
}
