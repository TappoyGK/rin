//! C ホストに借用させるエラー情報と、その文字列の所有を管理する。

use crate::error::{Error, ErrorKind};
use std::ffi::{CString, c_char};

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
}

impl From<ErrorKind> for RinErrorKind {
    fn from(kind: ErrorKind) -> Self {
        match kind {
            ErrorKind::Lex => Self::Lex,
            ErrorKind::Parse => Self::Parse,
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

pub(super) struct StoredError {
    pub view: RinError,
    // CString のバッファは移動しない。view が参照する間、所有を保持する。
    _message: CString,
    _module: Option<CString>,
    _backtrace: Option<CString>,
    _text: CString,
}

fn c_string(text: &str) -> CString {
    // 内部 NUL を可視化し、残りの情報も失わずに C 文字列へ変換する。
    CString::new(text.replace('\0', "\\0")).expect("NUL characters were escaped")
}

impl StoredError {
    pub fn from_error(err: Error) -> Self {
        let backtrace = err.format_backtrace();
        Self::new(
            err.kind.into(),
            &err.message,
            err.module(),
            err.at.line,
            err.at.col,
            if backtrace.is_empty() {
                None
            } else {
                Some(&backtrace)
            },
            &err.to_string(),
        )
    }

    pub fn host(message: &str) -> Self {
        Self::new(RinErrorKind::Host, message, None, 0, 0, None, message)
    }

    fn new(
        kind: RinErrorKind,
        message: &str,
        module: Option<&str>,
        line: usize,
        col: usize,
        backtrace: Option<&str>,
        text: &str,
    ) -> Self {
        let message = c_string(message);
        let module = module.map(c_string);
        let backtrace = backtrace.map(c_string);
        let text = c_string(text);
        Self {
            view: RinError {
                kind,
                message: message.as_ptr(),
                module: module.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                rule: std::ptr::null(),
                line,
                col,
                backtrace: backtrace.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                text: text.as_ptr(),
            },
            _message: message,
            _module: module,
            _backtrace: backtrace,
            _text: text,
        }
    }
}
