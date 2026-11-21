use std::fmt;

/// Source position (1-based line and column).
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

/// Resource limits for a single Rin evaluation.
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
    Runtime,
    TooManySteps,
    TooDeep,
    TooMuchMemory,
}

/// A single call frame recorded when a runtime error unwinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackFrame {
    pub module: Option<String>,
    pub name: Option<String>,
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

impl StackFrame {
    fn same_site(&self, other: &Self) -> bool {
        self.module == other.module
            && self.name == other.name
            && self.at == other.at
            && self.tail_calls == other.tail_calls
            && self.tail_target_module == other.tail_target_module
            && self.tail_target_name == other.tail_target_name
            && self.from_host == other.from_host
            && self.via_builtin == other.via_builtin
    }
}

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
                write!(f, ", called by {b} at {})", self.at)?;
            } else {
                write!(f, " at {})", self.at)?;
            }
            write!(f, " [tail calls elided: {}]", self.tail_calls)?;
        } else {
            fmt_fn_name(f, self.module.as_deref(), self.name.as_deref())?;
            if self.from_host {
                write!(f, " (called from host)")?;
            } else if let Some(b) = &self.via_builtin {
                write!(f, " (called by {b} at {})", self.at)?;
            } else {
                write!(f, " (at {})", self.at)?;
            }
        }
        if self.repeat_count > 1 {
            write!(f, " ×{}", self.repeat_count)?;
        }
        Ok(())
    }
}

const MAX_BACKTRACE_SIDE: usize = 32;

/// Error returned by lexing, parsing, or evaluating Rin code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub at: Span,
    pub message: String,
    module: Option<String>,
    backtrace: Vec<StackFrame>,
    omitted_frames: usize,
}

impl Error {
    pub fn new(kind: ErrorKind, at: Span, message: impl Into<String>) -> Self {
        Self {
            kind,
            at,
            message: message.into(),
            module: None,
            backtrace: Vec::new(),
            omitted_frames: 0,
        }
    }

    pub fn lex(at: Span, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Lex, at, message)
    }

    pub fn parse(at: Span, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Parse, at, message)
    }

    pub fn runtime(at: Span, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Runtime, at, message)
    }

    /// Returns the module name where the error occurred, if inside a module.
    pub fn module(&self) -> Option<&str> {
        self.module.as_deref()
    }

    pub(crate) fn set_module_if_none(&mut self, module: String) {
        if self.module.is_none() {
            self.module = Some(module);
        }
    }

    /// Returns the call frames from innermost callee to outermost caller.
    pub fn backtrace(&self) -> &[StackFrame] {
        &self.backtrace
    }

    /// Returns the number of middle call frames omitted when the stack exceeds the display cap.
    pub fn omitted_frames(&self) -> usize {
        self.omitted_frames
    }

    /// Formats the call backtrace lines without the leading error header.
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

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(m) = &self.module {
            write!(f, "{m} {}: {}", self.at, self.message)?;
        } else {
            write!(f, "{}: {}", self.at, self.message)?;
        }
        let bt = self.format_backtrace();
        if !bt.is_empty() {
            write!(f, "\n{bt}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}
