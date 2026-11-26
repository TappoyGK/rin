//! システムヒープを一切使わないインラインエラーバッファと借用エラービュー [`ErrorRef`] を提供する。

use std::fmt::{self, Write};

use super::{Error, ErrorKind, MAX_BACKTRACE_SIDE, Span, StackFrame, fmt_fn_name, fmt_site};

/// 内部処理で `ErrorStore` へエラーを書き込んだことを表す 0 バイトのマーカー。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ErrRecorded;

/// 固定長バイト配列に書き込むゼロアロケーション文字列バッファ。
#[derive(Clone, Copy)]
pub(crate) struct InlineBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> InlineBuf<N> {
    pub const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: `push_str` は常に UTF-8 文字境界で切り詰めるため有効な UTF-8。
        unsafe { std::str::from_utf8_unchecked(&self.buf[..self.len]) }
    }

    pub fn push_str(&mut self, s: &str) {
        let avail = N.saturating_sub(self.len);
        if avail == 0 {
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
    }

    /// `s` を追記し、その範囲 `(start, len)` を返す。
    pub fn intern_slice(&mut self, s: &str) -> (u16, u16) {
        let start = self.len;
        self.push_str(s);
        let len = self.len - start;
        (start as u16, len as u16)
    }

    #[inline]
    pub fn get_slice(&self, span: (u16, u16)) -> &str {
        let start = span.0 as usize;
        let end = start + span.1 as usize;
        // SAFETY: `intern_slice` は常に有効な UTF-8 境界を返す。
        unsafe { std::str::from_utf8_unchecked(&self.buf[start..end]) }
    }
}

impl<const N: usize> Write for InlineBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_str(s);
        Ok(())
    }
}

const NONE_STR: (u16, u16) = (u16::MAX, 0);

#[derive(Clone, Copy)]
struct StoredFrame {
    module: (u16, u16),
    name: (u16, u16),
    caller_module: (u16, u16),
    at: Span,
    tail_calls: usize,
    tail_target_module: (u16, u16),
    tail_target_name: (u16, u16),
    repeat_count: usize,
    from_host: bool,
    via_builtin: Option<&'static str>,
}

impl StoredFrame {
    const fn empty() -> Self {
        Self {
            module: NONE_STR,
            name: NONE_STR,
            caller_module: NONE_STR,
            at: Span { line: 0, col: 0 },
            tail_calls: 0,
            tail_target_module: NONE_STR,
            tail_target_name: NONE_STR,
            repeat_count: 0,
            from_host: false,
            via_builtin: None,
        }
    }
}

pub(crate) struct FrameInput<'a> {
    pub module: Option<&'a str>,
    pub name: Option<&'a str>,
    pub caller_module: Option<&'a str>,
    pub at: Span,
    pub tail_calls: usize,
    pub tail_target_module: Option<&'a str>,
    pub tail_target_name: Option<&'a str>,
    pub repeat_count: usize,
    pub from_host: bool,
    pub via_builtin: Option<&'static str>,
}

const MAX_FRAMES: usize = MAX_BACKTRACE_SIDE * 2;

/// システムヒープを使わず最新エラーと呼び出し履歴を保持するインラインストア。
pub(crate) struct ErrorStore {
    has_error: bool,
    kind: ErrorKind,
    at: Span,
    rule: &'static str,
    msg: InlineBuf<1024>,
    str_pool: InlineBuf<2048>,
    module: (u16, u16),
    module_resolved: bool,
    frames: [StoredFrame; MAX_FRAMES],
    frames_len: usize,
    omitted_frames: usize,
}

impl ErrorStore {
    pub const fn new() -> Self {
        Self {
            has_error: false,
            kind: ErrorKind::Runtime,
            at: Span { line: 0, col: 0 },
            rule: "",
            msg: InlineBuf::new(),
            str_pool: InlineBuf::new(),
            module: NONE_STR,
            module_resolved: false,
            frames: [StoredFrame::empty(); MAX_FRAMES],
            frames_len: 0,
            omitted_frames: 0,
        }
    }

    fn reset_header(&mut self, rule: &'static str, kind: ErrorKind, at: Span) {
        assert!(!rule.is_empty(), "error rule must not be empty");
        self.has_error = true;
        self.rule = rule;
        self.kind = kind;
        self.at = at;
        self.msg.clear();
        self.str_pool.clear();
        self.module = NONE_STR;
        self.module_resolved = false;
        self.frames_len = 0;
        self.omitted_frames = 0;
    }

    pub fn set_str(
        &mut self,
        rule: &'static str,
        kind: ErrorKind,
        at: Span,
        msg: &str,
    ) -> ErrRecorded {
        self.reset_header(rule, kind, at);
        self.msg.push_str(msg);
        ErrRecorded
    }

    pub fn set_fmt(
        &mut self,
        rule: &'static str,
        kind: ErrorKind,
        at: Span,
        args: fmt::Arguments<'_>,
    ) -> ErrRecorded {
        self.reset_header(rule, kind, at);
        let _ = self.msg.write_fmt(args);
        ErrRecorded
    }

    /// メッセージ追記用に `InlineBuf<1024>` への可変参照を返す。
    pub fn begin_msg(
        &mut self,
        rule: &'static str,
        kind: ErrorKind,
        at: Span,
    ) -> &mut InlineBuf<1024> {
        self.reset_header(rule, kind, at);
        &mut self.msg
    }

    fn intern_opt(&mut self, s: Option<&str>) -> (u16, u16) {
        match s {
            Some(text) => self.str_pool.intern_slice(text),
            None => NONE_STR,
        }
    }

    fn get_opt(&self, span: (u16, u16)) -> Option<&str> {
        if span.0 == u16::MAX {
            None
        } else {
            Some(self.str_pool.get_slice(span))
        }
    }

    pub fn resolve_module(&mut self, module: Option<&str>) {
        if !self.module_resolved {
            self.module_resolved = true;
            self.module = self.intern_opt(module);
        }
    }

    fn same_frame(&self, a: &StoredFrame, b: &FrameInput<'_>) -> bool {
        self.get_opt(a.module) == b.module
            && self.get_opt(a.name) == b.name
            && self.get_opt(a.caller_module) == b.caller_module
            && a.at == b.at
            && a.tail_calls == b.tail_calls
            && self.get_opt(a.tail_target_module) == b.tail_target_module
            && self.get_opt(a.tail_target_name) == b.tail_target_name
            && a.from_host == b.from_host
            && a.via_builtin == b.via_builtin
    }

    pub fn push_frame(&mut self, frame: FrameInput<'_>) {
        if self.frames_len > 0 && self.same_frame(&self.frames[self.frames_len - 1], &frame) {
            self.frames[self.frames_len - 1].repeat_count += frame.repeat_count;
            return;
        }
        let stored = StoredFrame {
            module: self.intern_opt(frame.module),
            name: self.intern_opt(frame.name),
            caller_module: self.intern_opt(frame.caller_module),
            at: frame.at,
            tail_calls: frame.tail_calls,
            tail_target_module: self.intern_opt(frame.tail_target_module),
            tail_target_name: self.intern_opt(frame.tail_target_name),
            repeat_count: frame.repeat_count,
            from_host: frame.from_host,
            via_builtin: frame.via_builtin,
        };
        if self.frames_len >= MAX_FRAMES {
            let removed = self.frames[MAX_BACKTRACE_SIDE];
            self.omitted_frames += removed.repeat_count;
            for i in MAX_BACKTRACE_SIDE..(MAX_FRAMES - 1) {
                self.frames[i] = self.frames[i + 1];
            }
            self.frames[MAX_FRAMES - 1] = stored;
        } else {
            self.frames[self.frames_len] = stored;
            self.frames_len += 1;
        }
    }

    pub fn as_ref(&self) -> ErrorRef<'_> {
        ErrorRef(self)
    }

    pub fn to_owned_error(&self) -> Error {
        let mut err = Error::new(self.rule, self.kind, self.at, self.msg.as_str());
        if self.module_resolved {
            err.resolve_module(self.get_opt(self.module).map(str::to_owned));
        }
        for i in 0..self.frames_len {
            let f = &self.frames[i];
            err.backtrace.push(StackFrame {
                module: self.get_opt(f.module).map(str::to_owned),
                name: self.get_opt(f.name).map(str::to_owned),
                caller_module: self.get_opt(f.caller_module).map(str::to_owned),
                at: f.at,
                tail_calls: f.tail_calls,
                tail_target_module: self.get_opt(f.tail_target_module).map(str::to_owned),
                tail_target_name: self.get_opt(f.tail_target_name).map(str::to_owned),
                repeat_count: f.repeat_count,
                from_host: f.from_host,
                via_builtin: f.via_builtin.map(str::to_owned),
            });
        }
        err.omitted_frames = self.omitted_frames;
        err
    }

    fn fmt_frame(&self, frame: &StoredFrame, f: &mut (impl fmt::Write + ?Sized)) -> fmt::Result {
        if frame.tail_calls > 0 {
            fmt_fn_name_write(
                f,
                self.get_opt(frame.tail_target_module),
                self.get_opt(frame.tail_target_name),
            )?;
            write!(f, " (tail-called from ")?;
            fmt_fn_name_write(f, self.get_opt(frame.module), self.get_opt(frame.name))?;
            if frame.from_host {
                write!(f, ", called from host)")?;
            } else if let Some(b) = frame.via_builtin {
                write!(f, ", called by {b} at ")?;
                fmt_site_write(f, self.get_opt(frame.caller_module), frame.at)?;
                write!(f, ")")?;
            } else {
                write!(f, " at ")?;
                fmt_site_write(f, self.get_opt(frame.caller_module), frame.at)?;
                write!(f, ")")?;
            }
            write!(f, " [tail calls elided: {}]", frame.tail_calls)?;
        } else {
            fmt_fn_name_write(f, self.get_opt(frame.module), self.get_opt(frame.name))?;
            if frame.from_host {
                write!(f, " (called from host)")?;
            } else if let Some(b) = frame.via_builtin {
                write!(f, " (called by {b} at ")?;
                fmt_site_write(f, self.get_opt(frame.caller_module), frame.at)?;
                write!(f, ")")?;
            } else {
                write!(f, " (at ")?;
                fmt_site_write(f, self.get_opt(frame.caller_module), frame.at)?;
                write!(f, ")")?;
            }
        }
        if frame.repeat_count > 1 {
            write!(f, " ×{}", frame.repeat_count)?;
        }
        Ok(())
    }
}

fn fmt_fn_name_write(
    f: &mut (impl fmt::Write + ?Sized),
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

fn fmt_site_write(
    f: &mut (impl fmt::Write + ?Sized),
    caller_module: Option<&str>,
    at: Span,
) -> fmt::Result {
    if let Some(m) = caller_module {
        write!(f, "{m} {at}")
    } else {
        write!(f, "{at}")
    }
}

/// システムヒープを使わない借用エラービュー。
///
/// [`fmt::Display`] は `fmt::Write` へ直接書き出し、システムヒープを確保しない。
#[derive(Clone, Copy)]
pub struct ErrorRef<'a>(&'a ErrorStore);

impl<'a> ErrorRef<'a> {
    #[inline]
    pub fn kind(&self) -> ErrorKind {
        self.0.kind
    }

    #[inline]
    pub fn at(&self) -> Span {
        self.0.at
    }

    #[inline]
    pub fn rule(&self) -> &'static str {
        self.0.rule
    }

    #[inline]
    pub fn message(&self) -> &'a str {
        self.0.msg.as_str()
    }

    #[inline]
    pub fn module(&self) -> Option<&'a str> {
        self.0.get_opt(self.0.module)
    }

    #[inline]
    pub fn backtrace_len(&self) -> usize {
        self.0.frames_len
    }

    #[inline]
    pub fn omitted_frames(&self) -> usize {
        self.0.omitted_frames
    }

    /// 先頭の見出し行を除いた呼び出し履歴を `f` へ直接書き出す（システムヒープ不使用）。
    pub fn write_backtrace(&self, f: &mut (impl fmt::Write + ?Sized)) -> fmt::Result {
        let mut wrote = false;
        for i in 0..self.0.frames_len {
            if i == MAX_BACKTRACE_SIDE && self.0.omitted_frames > 0 {
                if wrote {
                    f.write_char('\n')?;
                }
                write!(f, "  ... {} frames omitted", self.0.omitted_frames)?;
                wrote = true;
            }
            if wrote {
                f.write_char('\n')?;
            }
            f.write_str("  in ")?;
            self.0.fmt_frame(&self.0.frames[i], f)?;
            wrote = true;
        }
        Ok(())
    }

    /// 所有型の [`Error`] へ変換する。
    pub fn to_error(&self) -> Error {
        self.0.to_owned_error()
    }
}

impl fmt::Display for ErrorRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(m) = self.module() {
            write!(f, "{m} {}: {}", self.0.at, self.0.msg.as_str())?;
        } else {
            write!(f, "{}: {}", self.0.at, self.0.msg.as_str())?;
        }
        write!(f, " [{}]", self.0.rule)?;
        if self.0.frames_len > 0 {
            f.write_char('\n')?;
            self.write_backtrace(f)?;
        }
        Ok(())
    }
}

impl fmt::Debug for ErrorRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErrorRef")
            .field("kind", &self.0.kind)
            .field("at", &self.0.at)
            .field("rule", &self.0.rule)
            .field("message", &self.0.msg.as_str())
            .field("module", &self.module())
            .field("backtrace_len", &self.0.frames_len)
            .field("omitted_frames", &self.0.omitted_frames)
            .finish()
    }
}

// Keep unused imports warning-free for helpers shared with error.rs
#[allow(dead_code)]
fn _use_helpers() {
    let _ = (fmt_fn_name, fmt_site);
}
