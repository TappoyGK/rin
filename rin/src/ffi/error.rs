//! C ホストに借用させるエラー情報（システムヒープ不使用・インライン NUL 終端バッファ）。

use std::ffi::c_char;
use std::fmt::{self, Write};

use crate::error::{ErrorKind, ErrorRef, Span};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RinErrorKind {
    Lex = 0,
    Parse = 1,
    Runtime = 2,
    TooManySteps = 3,
    TooDeep = 4,
    TooMuchMemory = 5,
    Host = 6,
    Type = 7,
}

impl From<ErrorKind> for RinErrorKind {
    fn from(kind: ErrorKind) -> Self {
        match kind {
            ErrorKind::Lex => Self::Lex,
            ErrorKind::Parse => Self::Parse,
            ErrorKind::Type => Self::Type,
            ErrorKind::Runtime => Self::Runtime,
            ErrorKind::TooManySteps => Self::TooManySteps,
            ErrorKind::TooDeep => Self::TooDeep,
            ErrorKind::TooMuchMemory => Self::TooMuchMemory,
        }
    }
}

/// Machine が所有するエラーの借用ビュー。各文字列をホスト側で解放してはならない。
#[repr(C)]
pub struct RinError {
    pub kind: RinErrorKind,
    pub message: *const c_char,
    pub module: *const c_char,
    pub rule: *const c_char,
    pub line: usize,
    pub col: usize,
    pub backtrace: *const c_char,
    pub text: *const c_char,
}

/// 常に NUL 終端を保つ固定長インライン C 文字列バッファ。内部の `\0` は `\\0` に可視化する。
struct CInlineBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> CInlineBuf<N> {
    const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
        self.buf[0] = 0;
    }

    fn as_ptr(&self) -> *const c_char {
        self.buf.as_ptr().cast::<c_char>()
    }

    fn push_escaped(&mut self, s: &str) {
        let cap = N - 1;
        for chunk in s.split_inclusive('\0') {
            let (body, has_nul) = match chunk.strip_suffix('\0') {
                Some(b) => (b, true),
                None => (chunk, false),
            };
            self.push_raw_utf8(body, cap);
            if has_nul {
                self.push_raw_utf8("\\0", cap);
            }
        }
        self.buf[self.len] = 0;
    }

    fn push_raw_utf8(&mut self, s: &str, cap: usize) {
        let avail = cap.saturating_sub(self.len);
        if avail == 0 || s.is_empty() {
            return;
        }
        let bytes = s.as_bytes();
        let copy_len = if bytes.len() <= avail {
            bytes.len()
        } else {
            let mut end = avail;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            end
        };
        self.buf[self.len..self.len + copy_len].copy_from_slice(&bytes[..copy_len]);
        self.len += copy_len;
        self.buf[self.len] = 0;
    }
}

impl<const N: usize> Write for CInlineBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_escaped(s);
        Ok(())
    }
}

/// `Rin` 本体の内部にインライン配置されるエラーバッファ。
/// `Rin` はアロケータ上で移動しないため、`view` のポインタはバッファを直接指す。
pub(super) struct StoredError {
    pub view: RinError,
    rule: CInlineBuf<64>,
    message: CInlineBuf<1024>,
    module: CInlineBuf<128>,
    backtrace: CInlineBuf<2048>,
    text: CInlineBuf<3200>,
}

impl StoredError {
    pub const fn new_empty() -> Self {
        Self {
            view: RinError {
                kind: RinErrorKind::Runtime,
                message: std::ptr::null(),
                module: std::ptr::null(),
                rule: std::ptr::null(),
                line: 0,
                col: 0,
                backtrace: std::ptr::null(),
                text: std::ptr::null(),
            },
            rule: CInlineBuf::new(),
            message: CInlineBuf::new(),
            module: CInlineBuf::new(),
            backtrace: CInlineBuf::new(),
            text: CInlineBuf::new(),
        }
    }

    pub fn set_from_ref(&mut self, err: ErrorRef<'_>) {
        self.rule.clear();
        self.message.clear();
        self.module.clear();
        self.backtrace.clear();
        self.text.clear();

        self.rule.push_escaped(err.rule());
        self.message.push_escaped(err.message());
        let has_module = if let Some(m) = err.module() {
            self.module.push_escaped(m);
            true
        } else {
            false
        };
        let has_backtrace = if err.backtrace_len() > 0 {
            let _ = err.write_backtrace(&mut self.backtrace);
            true
        } else {
            false
        };
        let _ = write!(self.text, "{err}");

        self.view = RinError {
            kind: err.kind().into(),
            message: self.message.as_ptr(),
            module: if has_module {
                self.module.as_ptr()
            } else {
                std::ptr::null()
            },
            rule: self.rule.as_ptr(),
            line: err.at().line,
            col: err.at().col,
            backtrace: if has_backtrace {
                self.backtrace.as_ptr()
            } else {
                std::ptr::null()
            },
            text: self.text.as_ptr(),
        };
    }

    pub fn set_host(&mut self, message: &str) {
        self.set_simple(
            "host.argument",
            RinErrorKind::Host,
            Span::default(),
            message,
        );
    }

    pub fn set_simple(&mut self, rule: &str, kind: RinErrorKind, at: Span, message: &str) {
        self.rule.clear();
        self.message.clear();
        self.module.clear();
        self.backtrace.clear();
        self.text.clear();

        self.rule.push_escaped(rule);
        self.message.push_escaped(message);
        if kind == RinErrorKind::Host {
            let _ = write!(self.text, "{message} [{rule}]");
        } else {
            let _ = write!(self.text, "{at}: {message} [{rule}]");
        }

        self.view = RinError {
            kind,
            message: self.message.as_ptr(),
            module: std::ptr::null(),
            rule: self.rule.as_ptr(),
            line: at.line,
            col: at.col,
            backtrace: std::ptr::null(),
            text: self.text.as_ptr(),
        };
    }

    pub fn set_simple_fmt(
        &mut self,
        rule: &str,
        kind: RinErrorKind,
        at: Span,
        args: fmt::Arguments<'_>,
    ) {
        self.rule.clear();
        self.message.clear();
        self.module.clear();
        self.backtrace.clear();
        self.text.clear();

        self.rule.push_escaped(rule);
        let _ = self.message.write_fmt(args);
        let _ = write!(self.text, "{at}: ");
        let _ = self.text.write_fmt(args);
        let _ = write!(self.text, " [{rule}]");

        self.view = RinError {
            kind,
            message: self.message.as_ptr(),
            module: std::ptr::null(),
            rule: self.rule.as_ptr(),
            line: at.line,
            col: at.col,
            backtrace: std::ptr::null(),
            text: self.text.as_ptr(),
        };
    }
}
