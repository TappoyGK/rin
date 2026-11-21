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

/// Error returned by lexing, parsing, or evaluating Rin code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub at: Span,
    pub message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, at: Span, message: impl Into<String>) -> Self {
        Self {
            kind,
            at,
            message: message.into(),
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.message)
    }
}

impl std::error::Error for Error {}
