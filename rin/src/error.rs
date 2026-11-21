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
    pub name: Option<String>,
    pub at: Span,
}

/// Error returned by lexing, parsing, or evaluating Rin code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub at: Span,
    pub message: String,
    backtrace: Vec<StackFrame>,
}

impl Error {
    pub fn new(kind: ErrorKind, at: Span, message: impl Into<String>) -> Self {
        Self {
            kind,
            at,
            message: message.into(),
            backtrace: Vec::new(),
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

    /// Returns the call frames from innermost callee to outermost caller.
    pub fn backtrace(&self) -> &[StackFrame] {
        &self.backtrace
    }

    pub(crate) fn push_frame(&mut self, name: Option<String>, at: Span) {
        self.backtrace.push(StackFrame { name, at });
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.message)?;
        for frame in &self.backtrace {
            let name = frame.name.as_deref().unwrap_or("<lambda>");
            write!(f, "\n  in {name} (at {})", frame.at)?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}
