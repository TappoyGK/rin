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
            if self.peek() == Some(b'/') && self.peek_at(1) == Some(b'/') {
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
