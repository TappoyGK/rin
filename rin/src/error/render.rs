//! ホストが提供したソースを使う診断表示。評価器はソースを保持しない。

use super::{Error, MAX_BACKTRACE_SIDE, Span};
use std::fmt;

impl Error {
    /// エラーの発生元のソースを添えて表示する。
    ///
    /// 呼び出し履歴では [`Self::module`] と同じモジュールの位置だけにソースを添える。
    /// 複数のモジュールには [`Self::render_with`] を使う。
    ///
    /// ```
    /// let source = "(/ 1 0)";
    /// let error = rin::eval(source).unwrap_err();
    /// assert_eq!(error.render(source),
    ///     "1:1: division by zero [op.arith]\n  |\n1 | (/ 1 0)\n  | ^");
    /// ```
    pub fn render(&self, source: &str) -> String {
        self.render_with(|module| (module == self.module()).then_some(source))
    }

    /// モジュール名からソースを取得し、発生位置と呼び出し位置に該当行を添える。
    ///
    /// `None` はモジュール外のソースを表す。取得できないソースには `None` を返す。
    /// 履歴には呼び出し先ではなく呼び出し元のモジュールのソースを使う。
    /// 同じモジュールについて複数回呼び出す場合があるが、各表示位置につき 1 回だけ取得する。
    pub fn render_with<'s>(&self, source: impl FnMut(Option<&str>) -> Option<&'s str>) -> String {
        let mut out = String::new();
        self.render_with_to(&mut out, source)
            .expect("writing to String cannot fail");
        out
    }

    /// [`Self::render`] と同じ表示を `fmt::Write` に書き込む。
    pub fn render_to(&self, out: &mut (impl fmt::Write + ?Sized), source: &str) -> fmt::Result {
        self.render_with_to(out, |module| (module == self.module()).then_some(source))
    }

    /// [`Self::render_with`] と同じ表示を書き込む。書き込みエラーは直ちに返す。
    ///
    /// ソースのない位置・ホストからの入口には、既存の診断と履歴だけを表示する。
    /// 行・列が範囲外、列が UTF-8 文字の途中、または `0:0` の場合も同様。
    /// 列は 1 始まりの UTF-8 バイト位置。タブはソース行の先頭から 4 桁区切りで
    /// 展開し、その他の文字は仕様の範囲表による表示幅を使う。
    pub fn render_with_to<'s>(
        &self,
        out: &mut (impl fmt::Write + ?Sized),
        mut source: impl FnMut(Option<&str>) -> Option<&'s str>,
    ) -> fmt::Result {
        self.write_header(out)?;
        // 幅の決定と出力で同じ取得結果を使い、FnMut を二度呼ばない。
        let mut resolve = |module: Option<&str>, at: Span| {
            if at.line == 0 || at.col == 0 {
                return None;
            }
            Snippet::new(source(module)?, at)
        };
        let primary = resolve(self.module(), self.at);
        let frames: Vec<_> = self
            .backtrace
            .iter()
            .map(|frame| {
                if frame.from_host {
                    None
                } else {
                    resolve(frame.caller_module.as_deref(), frame.at)
                }
            })
            .collect();
        let max_line = primary
            .iter()
            .chain(frames.iter().flatten())
            .map(|snippet| snippet.number)
            .max()
            .unwrap_or(1);
        let digits = max_line.ilog10() as usize + 1;
        snippet(out, primary.as_ref(), digits)?;
        for (i, frame) in self.backtrace.iter().enumerate() {
            if i == MAX_BACKTRACE_SIDE && self.omitted_frames > 0 {
                write!(out, "\n  ... {} frames omitted", self.omitted_frames)?;
            }
            write!(out, "\n  in {frame}")?;
            snippet(out, frames[i].as_ref(), digits)?;
        }
        Ok(())
    }
}

struct Snippet<'s> {
    line: &'s str,
    prefix: &'s str,
    number: usize,
}

impl<'s> Snippet<'s> {
    fn new(source: &'s str, at: Span) -> Option<Self> {
        let line = source.split('\n').nth(at.line - 1)?;
        // CRLF の CR を表示に含めない。最終行の末尾 CR も同じく扱う。
        let line = line.strip_suffix('\r').unwrap_or(line);
        let prefix = line.get(..at.col - 1)?;
        Some(Self {
            line,
            prefix,
            number: at.line,
        })
    }
}

fn snippet(
    out: &mut (impl fmt::Write + ?Sized),
    snippet: Option<&Snippet<'_>>,
    digits: usize,
) -> fmt::Result {
    let Some(snippet) = snippet else {
        return Ok(());
    };
    let caret = write_line(&mut NullWriter, snippet.prefix)?;
    write!(out, "\n{:digits$} |\n{:>digits$} | ", "", snippet.number)?;
    write_line(out, snippet.line)?;
    write!(out, "\n{:digits$} | {:caret$}^", "", "")
}

// 表示と位置計算に同じタブ展開・エスケープ規則を使う。
fn write_line(out: &mut (impl fmt::Write + ?Sized), line: &str) -> Result<usize, fmt::Error> {
    let mut start = 0;
    let mut width = 0;
    for (i, c) in line.char_indices() {
        if c == '\t' || c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
            let segment = &line[start..i];
            out.write_str(segment)?;
            width += segment.chars().map(char_width).sum::<usize>();
            if c == '\t' {
                let spaces = 4 - width % 4;
                out.write_str(&"    "[..spaces])?;
                width += spaces;
            } else {
                for escaped in c.escape_default() {
                    out.write_char(escaped)?;
                    width += 1;
                }
            }
            start = i + c.len_utf8();
        }
    }
    let segment = &line[start..];
    out.write_str(segment)?;
    Ok(width + segment.chars().map(char_width).sum::<usize>())
}

// [eval.snippet] の小さな範囲表。ゼロ幅を先に判定する。
fn char_width(c: char) -> usize {
    match c as u32 {
        0x0300..=0x036f | 0x200b..=0x200f | 0x3099..=0x309a | 0xfe00..=0xfe0f => 0,
        0x1100..=0x115f
        | 0x2e80..=0x303e
        | 0x3041..=0x33ff
        | 0x3400..=0x4dbf
        | 0x4e00..=0x9fff
        | 0xa000..=0xa4cf
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe30..=0xfe4f
        | 0xff00..=0xff60
        | 0xffe0..=0xffe6
        | 0x1f300..=0x1f64f
        | 0x1f900..=0x1f9ff
        | 0x20000..=0x2fffd
        | 0x30000..=0x3fffd => 2,
        _ => 1,
    }
}

struct NullWriter;

impl fmt::Write for NullWriter {
    fn write_str(&mut self, _: &str) -> fmt::Result {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StackFrame;

    #[test]
    fn omitted_frames_keep_their_place_between_source_snippets() {
        // Exercise rendering without requiring deep recursion in the VM.
        let mut error = Error::runtime("expr.name", Span::new(1, 1), "broken");
        for i in 0..80 {
            error.push_frame(StackFrame {
                module: None,
                name: Some(format!("f{i}")),
                caller_module: None,
                at: Span::new(1, 1),
                tail_calls: 0,
                tail_target_module: None,
                tail_target_name: None,
                repeat_count: 1,
                from_host: false,
                via_builtin: None,
            });
        }
        assert_eq!(error.omitted_frames(), 16);
        let rendered = error.render("broken");
        assert_eq!(rendered.matches(" frames omitted").count(), 1);
        assert!(rendered.contains("\n  ... 16 frames omitted\n  in f48 (at 1:1)\n  |"));
        assert_eq!(rendered.matches(" |\n").count(), 65);
        assert_eq!(error.render_with(|_| None), error.to_string());
    }
}
