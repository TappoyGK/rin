//! 構文木からの整形出力。
use super::*;

impl Cst<'_> {
    // 幅だけを保持し、平坦な出力は原文のリテラルと子ノードから直接書く。
    fn render_flat(&self, id: usize, out: &mut Output) {
        let (items, group) = match &self.nodes[id].kind {
            Kind::Atom(text) => {
                out.write(text);
                return;
            }
            Kind::Sequence(items) => (items, None),
            Kind::Group {
                open,
                close,
                style,
                items,
            } => (items, Some((*open, *close, *style))),
            Kind::Comment { .. } => unreachable!(),
        };
        if let Some((open, _, _)) = group {
            out.write(&open.to_string());
            if open == '{' && !items.is_empty() {
                out.write(" ");
            }
        }
        let data =
            group.is_some_and(|(_, _, s)| matches!(s, Style::Data | Style::Tuple | Style::Fields));
        for (i, &child) in items.iter().enumerate() {
            if i > 0 {
                if data {
                    out.write(", ");
                } else if self.space(items[i - 1], child) {
                    out.write(" ");
                }
            }
            self.render_flat(child, out);
        }
        if let Some((open, close, style)) = group {
            if matches!(style, Style::Tuple) && items.len() == 1 {
                out.write(",");
            }
            if open == '{' && !items.is_empty() {
                out.write(" ");
            }
            out.write(&close.to_string());
        }
    }

    fn definition(&self, id: usize) -> bool {
        let start = self.nodes[id].range.start;
        self.tokens.get(start+1).is_some_and(|(t,_)| matches!(&t.kind, TokenKind::Ident(n) if matches!(n.as_str(), "def" | "defn" | "deftest")))
            && self.tokens.get(start).is_some_and(|(t,_)| t.kind==TokenKind::LParen)
    }
    pub(in crate::fmt) fn render_root(&self, out: &mut Output) {
        let Kind::Group { items, .. } = &self.nodes[self.root].kind else {
            unreachable!()
        };
        let mut previous_definition = false;
        for &id in items {
            let node = &self.nodes[id];
            if let Kind::Comment { text, inline } = node.kind {
                if !inline {
                    out.line(0, node.blank || previous_definition);
                }
                self.comment(text, inline, 0, out);
                if !inline {
                    previous_definition = false;
                }
                continue;
            }
            let definition = self.definition(id);
            out.line(0, node.blank || (previous_definition && definition));
            self.render(id, 0, 0, out);
            previous_definition = definition;
        }
    }
    fn comment(&self, text: &str, inline: bool, indent: usize, out: &mut Output) {
        if inline && !out.at_start() {
            out.write(" ");
        } else {
            out.line(indent, false);
        }
        let semis = text.bytes().take_while(|b| *b == b';').count();
        out.write(&text[..semis]);
        let rest = &text[semis..];
        if !rest.is_empty() {
            if !rest.starts_with(char::is_whitespace) {
                out.write(" ");
            }
            out.write(rest);
        }
        out.line(indent, false);
    }
    fn render(&self, id: usize, indent: usize, reserve: usize, out: &mut Output) {
        let node = &self.nodes[id];
        if let Some(flat) = &node.flat
            && out.column() + flat + reserve <= WIDTH
        {
            self.render_flat(id, out);
            return;
        }
        match &node.kind {
            Kind::Atom(text) => out.write(text),
            Kind::Comment { text, inline } => self.comment(text, *inline, indent, out),
            Kind::Sequence(items) => {
                let mut previous = None;
                for &part in items {
                    if let Kind::Comment { text, inline } = self.nodes[part].kind {
                        self.comment(text, inline, indent, out);
                        previous = None;
                        continue;
                    }
                    if let Some(prev) = previous
                        && self.space(prev, part)
                        && !out.at_start()
                    {
                        out.write(" ");
                    }
                    self.render(
                        part,
                        indent,
                        if items.last() == Some(&part) {
                            reserve
                        } else {
                            0
                        },
                        out,
                    );
                    previous = Some(part);
                }
            }
            Kind::Group {
                open,
                close,
                style,
                items,
            } => {
                let start = out.column();
                out.write(&open.to_string());
                let continuation = match style {
                    Style::Body(_) | Style::Arm | Style::Fields | Style::Data | Style::Tuple => {
                        start + 2
                    }
                    _ => start + 1,
                };
                let mut align = continuation;
                let mut count = 0;
                let mut previous = None;
                let data = matches!(style, Style::Data | Style::Tuple | Style::Fields);
                let total = items
                    .iter()
                    .filter(|&&i| !matches!(self.nodes[i].kind, Kind::Comment { .. }))
                    .count();
                for &part in items {
                    let child = &self.nodes[part];
                    if let Kind::Comment { text, inline } = child.kind {
                        if child.blank {
                            out.line(align, true);
                        }
                        self.comment(text, inline, align, out);
                        previous = None;
                        continue;
                    }
                    let hard_break = match style {
                        Style::Body(header) => count >= *header,
                        Style::Arm => count >= 1,
                        Style::Fields => true,
                        Style::Bindings | Style::Data | Style::Tuple => count > 0,
                        Style::Call | Style::If => count > 1,
                        Style::Params => false,
                        Style::Root => unreachable!(),
                    };
                    let space = previous.is_some_and(|prev| self.space(prev, part));
                    let must_attach = previous.is_some_and(|_| !space)
                        && matches!(
                            self.tokens[child.range.start].0.kind,
                            TokenKind::LParen | TokenKind::LBracket
                        );
                    let child_reserve = 1 + if count + 1 == total { reserve } else { 0 };
                    let overflow = child.flat.as_ref().is_some_and(|flat| {
                        out.column() + usize::from(space) + flat + child_reserve > WIDTH
                    });
                    if count == 1 && matches!(style, Style::Call | Style::If) {
                        align = if overflow {
                            start + 2
                        } else {
                            out.column() + usize::from(space)
                        };
                    }
                    if !must_attach && (hard_break || child.blank || overflow && previous.is_some())
                    {
                        out.line(align, child.blank);
                    } else if space && !out.at_start() {
                        out.write(" ");
                    }
                    self.render(part, align, child_reserve, out);
                    count += 1;
                    if data && (count < total || matches!(style, Style::Tuple) && total == 1) {
                        out.write(",");
                    }
                    previous = Some(part);
                }
                if out.at_start() {
                    out.indent = start;
                }
                out.write(&close.to_string());
            }
        }
    }
}

#[derive(Default)]
pub(in crate::fmt) struct Output {
    text: String,
    col: usize,
    indent: usize,
}
impl Output {
    fn at_start(&self) -> bool {
        self.text.is_empty() || self.text.ends_with('\n')
    }
    fn column(&self) -> usize {
        if self.at_start() {
            self.indent
        } else {
            self.col
        }
    }
    fn write(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.at_start() {
            self.text.push_str(&" ".repeat(self.indent));
            self.col = self.indent;
        }
        self.text.push_str(text);
        self.col = if let Some((_, tail)) = text.rsplit_once('\n') {
            tail.chars().count()
        } else {
            self.col + text.chars().count()
        };
    }
    fn line(&mut self, indent: usize, blank: bool) {
        if !self.text.is_empty() {
            if !self.text.ends_with('\n') {
                self.text.push('\n');
            }
            if blank && !self.text.ends_with("\n\n") {
                self.text.push('\n');
            }
        }
        self.indent = indent;
        self.col = indent;
    }
    pub(in crate::fmt) fn finish(mut self) -> String {
        while self.text.ends_with('\n') {
            self.text.pop();
        }
        self.text.push('\n');
        self.text
    }
}
