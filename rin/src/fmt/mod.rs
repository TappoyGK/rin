//! コメントとリテラルを保持する、設定不要のソース整形。
mod syntax;

use crate::ast::Expr;
use crate::lexer::{Token, TokenKind, tokenize_lossless};
use crate::parser::{ParseScratch, ParseScratchGuard, parse_tests};
use crate::{Error, Span};
use std::ops::Range;
use syntax::{Plan, Style, Syntax, equivalent};

const WIDTH: usize = 100;

/// ソースを2桁字下げ・100文字幅で整形する。
/// 構文エラー、または整形前後のASTが異なる場合はエラーを返す。
/// リテラル内部は原文を保ち、コメントの前後にあるトークンの順序を変えない。
pub fn format(source: &str) -> Result<String, Error> {
    let before = parse(source)?;
    let tokens = tokenize_lossless(source)?;
    let syntax = Syntax::new(&tokens, &before);
    let cst = Cst::build(source, &syntax);
    let mut out = Output::default();
    cst.render_root(&mut out);
    let output = out.finish();
    let after = parse(&output).map_err(|_| {
        Error::parse(
            "fmt.preserve",
            Span::default(),
            "formatting changed the source syntax",
        )
    })?;
    if !equivalent(&before, &after) {
        return Err(Error::parse(
            "fmt.preserve",
            Span::default(),
            "formatting changed the source AST",
        ));
    }
    Ok(output)
}

fn parse(source: &str) -> Result<Vec<Expr>, Error> {
    let mut scratch = ParseScratch::new();
    let mut guard = ParseScratchGuard::new(&mut scratch);
    if let Err(error) = guard.parse(source) {
        if matches!(error.rule(), "test.deftest" | "test.check") {
            crate::test::validate_source(source)?;
        } else {
            return Err(error);
        }
    }
    parse_tests(source)
}

enum Kind<'a> {
    Atom(&'a str),
    Comment {
        text: &'a str,
        inline: bool,
    },
    Sequence(Vec<usize>),
    Group {
        open: char,
        close: char,
        style: Style,
        items: Vec<usize>,
    },
}
struct Node<'a> {
    kind: Kind<'a>,
    range: Range<usize>,
    blank: bool,
    flat: Option<String>,
}
struct Cst<'a> {
    nodes: Vec<Node<'a>>,
    root: usize,
    tokens: &'a [(Token, Range<usize>)],
}
struct Frame {
    start: usize,
    blank: bool,
    items: Vec<usize>,
}

impl<'a> Cst<'a> {
    fn build(source: &'a str, syntax: &Syntax<'a>) -> Self {
        let mut this = Self {
            nodes: Vec::new(),
            root: 0,
            tokens: syntax.tokens,
        };
        let mut frames = vec![Frame {
            start: syntax.tokens.len(),
            blank: false,
            items: Vec::new(),
        }];
        let mut previous = 0;
        for i in 0..=syntax.tokens.len() {
            let next = syntax
                .tokens
                .get(i)
                .map_or(source.len(), |(_, range)| range.start);
            let blank = this.trivia(
                source,
                previous..next,
                i,
                &mut frames.last_mut().unwrap().items,
            );
            if i == syntax.tokens.len() {
                break;
            }
            let (token, range) = &syntax.tokens[i];
            match token.kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => frames.push(Frame {
                    start: i,
                    blank,
                    items: Vec::new(),
                }),
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    let frame = frames.pop().unwrap();
                    let open = match token.kind {
                        TokenKind::RParen => '(',
                        TokenKind::RBracket => '[',
                        _ => '{',
                    };
                    let close = match token.kind {
                        TokenKind::RParen => ')',
                        TokenKind::RBracket => ']',
                        _ => '}',
                    };
                    let start = frame.start;
                    let node = this.group(frame, i + 1, open, close, syntax.plans.get(&start));
                    frames.last_mut().unwrap().items.push(node);
                }
                TokenKind::Comma => {} // 各データ要素の間に正規化して付け直す。
                _ => {
                    let node = this.add(Kind::Atom(&source[range.clone()]), i..i + 1, blank);
                    frames.last_mut().unwrap().items.push(node);
                }
            }
            previous = range.end;
        }
        let frame = frames.pop().unwrap();
        this.root = this.group(
            frame,
            syntax.tokens.len(),
            '\0',
            '\0',
            syntax.plans.get(&syntax.tokens.len()),
        );
        this
    }

    fn trivia(
        &mut self,
        source: &'a str,
        gap: Range<usize>,
        index: usize,
        items: &mut Vec<usize>,
    ) -> bool {
        let mut cursor = gap.start;
        let mut previous = gap.start;
        while cursor < gap.end {
            if source.as_bytes()[cursor] != b';' {
                cursor += 1;
                continue;
            }
            let end = source[cursor..gap.end]
                .find('\n')
                .map_or(gap.end, |n| cursor + n);
            let text = source[cursor..end]
                .strip_suffix('\r')
                .unwrap_or(&source[cursor..end]);
            let prefix = &source[previous..cursor];
            let inline = previous > 0 && !prefix.contains('\n');
            let blank = prefix.bytes().filter(|b| *b == b'\n').count() > 1;
            let node = self.add(Kind::Comment { text, inline }, index..index, blank);
            items.push(node);
            previous = end;
            cursor = end;
        }
        source[previous..gap.end]
            .bytes()
            .filter(|b| *b == b'\n')
            .count()
            > 1
    }

    fn group(
        &mut self,
        frame: Frame,
        end: usize,
        open: char,
        close: char,
        plan: Option<&Plan>,
    ) -> usize {
        let style = plan.map_or(Style::Call, |p| p.style);
        let mut items = Vec::new();
        let mut i = 0;
        while i < frame.items.len() {
            let id = frame.items[i];
            let part = plan.and_then(|p| {
                p.parts
                    .binary_search_by_key(&self.nodes[id].range.start, |r| r.start)
                    .ok()
                    .map(|i| &p.parts[i])
            });
            if let Some(part) =
                part.filter(|_| !matches!(self.nodes[id].kind, Kind::Comment { .. }))
            {
                let first = i;
                while i < frame.items.len() && self.nodes[frame.items[i]].range.start < part.end {
                    i += 1;
                }
                if i - first == 1 {
                    items.push(id);
                } else {
                    let node = self.add(
                        Kind::Sequence(frame.items[first..i].to_vec()),
                        part.clone(),
                        self.nodes[id].blank,
                    );
                    items.push(node);
                }
            } else {
                items.push(id);
                i += 1;
            }
        }
        self.add(
            Kind::Group {
                open,
                close,
                style,
                items,
            },
            frame.start..end,
            frame.blank,
        )
    }

    fn add(&mut self, kind: Kind<'a>, range: Range<usize>, blank: bool) -> usize {
        let flat = match &kind {
            Kind::Atom(text)
                if !text.contains(['\n', '\r', '\t']) && text.chars().count() <= WIDTH =>
            {
                Some((*text).into())
            }
            Kind::Sequence(items) => self.flat_items(items, None),
            Kind::Group {
                open,
                close,
                style,
                items,
            } if !matches!(style, Style::Root) => {
                self.flat_items(items, Some((*open, *close, *style)))
            }
            _ => None,
        };
        self.nodes.push(Node {
            kind,
            range,
            blank,
            flat,
        });
        self.nodes.len() - 1
    }

    fn space(&self, left: usize, right: usize) -> bool {
        let a = &self.nodes[left];
        let b = &self.nodes[right];
        let previous = &self.tokens[a.range.end - 1].0.kind;
        let next = &self.tokens[b.range.start].0;
        if matches!(previous, TokenKind::Dot)
            || matches!(next.kind, TokenKind::Dot | TokenKind::Colon)
        {
            return false;
        }
        if matches!(next.kind, TokenKind::LParen | TokenKind::LBracket) {
            if matches!(
                b.kind,
                Kind::Group {
                    style: Style::Params | Style::Bindings | Style::Arm,
                    ..
                }
            ) {
                return true;
            }
            return !next.attached;
        }
        true
    }

    fn flat_items(&self, items: &[usize], group: Option<(char, char, Style)>) -> Option<String> {
        let mut text = String::new();
        if let Some((open, _, _)) = group {
            text.push(open);
            if open == '{' && !items.is_empty() {
                text.push(' ');
            }
        }
        let data =
            group.is_some_and(|(_, _, s)| matches!(s, Style::Data | Style::Tuple | Style::Fields));
        for (i, &id) in items.iter().enumerate() {
            let node = &self.nodes[id];
            if node.blank {
                return None;
            }
            let flat = node.flat.as_ref()?;
            if i > 0 {
                if data {
                    text.push_str(", ");
                } else if self.space(items[i - 1], id) {
                    text.push(' ');
                }
            }
            text.push_str(flat);
            if text.chars().count() > WIDTH {
                return None;
            }
        }
        if let Some((open, close, style)) = group {
            if matches!(style, Style::Tuple) && items.len() == 1 {
                text.push(',');
            }
            if open == '{' && !items.is_empty() {
                text.push(' ');
            }
            text.push(close);
        }
        (text.chars().count() <= WIDTH).then_some(text)
    }

    fn definition(&self, id: usize) -> bool {
        let start = self.nodes[id].range.start;
        self.tokens.get(start+1).is_some_and(|(t,_)| matches!(&t.kind, TokenKind::Ident(n) if matches!(n.as_str(), "def" | "defn" | "deftest")))
            && self.tokens.get(start).is_some_and(|(t,_)| t.kind==TokenKind::LParen)
    }
    fn render_root(&self, out: &mut Output) {
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
            && out.column() + flat.chars().count() + reserve <= WIDTH
        {
            out.write(flat);
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
                        out.column() + usize::from(space) + flat.chars().count() + child_reserve
                            > WIDTH
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
struct Output {
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
    fn finish(mut self) -> String {
        while self.text.ends_with('\n') {
            self.text.pop();
        }
        self.text.push('\n');
        self.text
    }
}
