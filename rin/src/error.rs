//! ソース位置、資源上限、エラー型、および呼び出し履歴フレームを定義する。

#[cfg(feature = "std-value")]
mod render;
mod store;

pub use store::ErrorRef;
#[allow(unused_imports)]
pub(crate) use store::{ErrRecorded, ErrorStore, FrameInput, InlineBuf};

use std::fmt;

/// ソース内の 1 始まりの行・UTF-8 バイト単位の列位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

impl Span {
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// 1 回の評価における資源上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub steps: usize,
    pub depth: usize,
    pub memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            steps: 100_000,
            depth: 200,
            memory_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Lex,
    Parse,
    Type,
    Runtime,
    TooManySteps,
    TooDeep,
    TooMuchMemory,
}

/// 実行時エラーまたは資源上限超過の展開時に記録する呼び出しフレーム。
#[cfg(feature = "std-value")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackFrame {
    pub module: Option<String>,
    pub name: Option<String>,
    pub caller_module: Option<String>,
    pub at: Span,
    pub tail_calls: usize,
    pub tail_target_module: Option<String>,
    pub tail_target_name: Option<String>,
    pub repeat_count: usize,
    pub from_host: bool,
    pub via_builtin: Option<String>,
}

fn fmt_fn_name(
    f: &mut fmt::Formatter<'_>,
    module: Option<&str>,
    name: Option<&str>,
) -> fmt::Result {
    match (module, name) {
        (Some(m), Some(n)) => write!(f, "{m}.{n}"),
        (Some(m), None) => write!(f, "{m}.<lambda>"),
        (None, Some(n)) => write!(f, "{n}"),
        (None, None) => write!(f, "<lambda>"),
    }
}

fn fmt_site(f: &mut fmt::Formatter<'_>, caller_module: Option<&str>, at: Span) -> fmt::Result {
    if let Some(m) = caller_module {
        write!(f, "{m} {at}")
    } else {
        write!(f, "{at}")
    }
}

#[cfg(all(test, feature = "std-value"))]
impl StackFrame {
    fn same_site(&self, other: &Self) -> bool {
        self.module == other.module
            && self.name == other.name
            && self.caller_module == other.caller_module
            && self.at == other.at
            && self.tail_calls == other.tail_calls
            && self.tail_target_module == other.tail_target_module
            && self.tail_target_name == other.tail_target_name
            && self.from_host == other.from_host
            && self.via_builtin == other.via_builtin
    }
}

#[cfg(feature = "std-value")]
impl fmt::Display for StackFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.tail_calls > 0 {
            fmt_fn_name(
                f,
                self.tail_target_module.as_deref(),
                self.tail_target_name.as_deref(),
            )?;
            write!(f, " (tail-called from ")?;
            fmt_fn_name(f, self.module.as_deref(), self.name.as_deref())?;
            if self.from_host {
                write!(f, ", called from host)")?;
            } else if let Some(b) = &self.via_builtin {
                write!(f, ", called by {b} at ")?;
                fmt_site(f, self.caller_module.as_deref(), self.at)?;
                write!(f, ")")?;
            } else {
                write!(f, " at ")?;
                fmt_site(f, self.caller_module.as_deref(), self.at)?;
                write!(f, ")")?;
            }
            write!(f, " [tail calls elided: {}]", self.tail_calls)?;
        } else {
            fmt_fn_name(f, self.module.as_deref(), self.name.as_deref())?;
            if self.from_host {
                write!(f, " (called from host)")?;
            } else if let Some(b) = &self.via_builtin {
                write!(f, " (called by {b} at ")?;
                fmt_site(f, self.caller_module.as_deref(), self.at)?;
                write!(f, ")")?;
            } else {
                write!(f, " (at ")?;
                fmt_site(f, self.caller_module.as_deref(), self.at)?;
                write!(f, ")")?;
            }
        }
        if self.repeat_count > 1 {
            write!(f, " ×{}", self.repeat_count)?;
        }
        Ok(())
    }
}

const MAX_BACKTRACE_SIDE: usize = 32;

/// 字句解析・構文解析・コンパイル・評価で返すエラー。
#[cfg(feature = "std-value")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub at: Span,
    pub message: String,
    rule: &'static str,
    module: Option<String>,
    module_resolved: bool,
    backtrace: Vec<StackFrame>,
    omitted_frames: usize,
}

#[cfg(feature = "std-value")]
impl Error {
    /// 規則名を指定してエラーを作る。
    ///
    /// # Panics
    /// `rule` が空の場合は panic する。
    pub fn new(rule: &'static str, kind: ErrorKind, at: Span, message: impl Into<String>) -> Self {
        assert!(!rule.is_empty(), "error rule must not be empty");
        Self {
            rule,
            kind,
            at,
            message: message.into(),
            module: None,
            module_resolved: false,
            backtrace: Vec::new(),
            omitted_frames: 0,
        }
    }

    pub fn lex(rule: &'static str, at: Span, message: impl Into<String>) -> Self {
        Self::new(rule, ErrorKind::Lex, at, message)
    }

    pub fn parse(rule: &'static str, at: Span, message: impl Into<String>) -> Self {
        Self::new(rule, ErrorKind::Parse, at, message)
    }

    pub fn type_err(rule: &'static str, at: Span, message: impl Into<String>) -> Self {
        Self::new(rule, ErrorKind::Type, at, message)
    }

    pub fn runtime(rule: &'static str, at: Span, message: impl Into<String>) -> Self {
        Self::new(rule, ErrorKind::Runtime, at, message)
    }

    /// 根拠となる仕様の規則名（角括弧を含まない）。
    pub fn rule(&self) -> &'static str {
        self.rule
    }

    /// エラーが発生したモジュール名を返す（モジュール外なら `None`）。
    pub fn module(&self) -> Option<&str> {
        self.module.as_deref()
    }

    pub(crate) fn resolve_module(&mut self, module: Option<String>) {
        if !self.module_resolved {
            self.module_resolved = true;
            self.module = module;
        }
    }

    /// 最も内側の呼び出し先から外側の呼び出し元へ向かう順にフレーム一覧を返す。
    pub fn backtrace(&self) -> &[StackFrame] {
        &self.backtrace
    }

    /// 保持上限を超えた場合に中央から省略されたフレーム数を返す。
    pub fn omitted_frames(&self) -> usize {
        self.omitted_frames
    }

    /// 先頭の見出し行を除いた呼び出し履歴を整形して返す。
    pub fn format_backtrace(&self) -> String {
        let mut out = String::new();
        for (i, frame) in self.backtrace.iter().enumerate() {
            if i == MAX_BACKTRACE_SIDE && self.omitted_frames > 0 {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("  ... {} frames omitted", self.omitted_frames));
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("  in {frame}"));
        }
        out
    }

    fn write_header(&self, f: &mut (impl fmt::Write + ?Sized)) -> fmt::Result {
        if let Some(m) = &self.module {
            write!(f, "{m} {}: {}", self.at, self.message)?;
        } else {
            write!(f, "{}: {}", self.at, self.message)?;
        }
        write!(f, " [{}]", self.rule)
    }

    #[cfg(test)]
    pub(crate) fn push_frame(&mut self, frame: StackFrame) {
        if let Some(last) = self.backtrace.last_mut()
            && last.same_site(&frame)
        {
            last.repeat_count += frame.repeat_count;
            return;
        }
        if self.backtrace.len() >= MAX_BACKTRACE_SIDE * 2 {
            let removed = self.backtrace.remove(MAX_BACKTRACE_SIDE);
            self.omitted_frames += removed.repeat_count;
        }
        self.backtrace.push(frame);
    }
}

#[cfg(feature = "std-value")]
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_header(f)?;
        let bt = self.format_backtrace();
        if !bt.is_empty() {
            write!(f, "\n{bt}")?;
        }
        Ok(())
    }
}

#[cfg(feature = "std-value")]
impl std::error::Error for Error {}
