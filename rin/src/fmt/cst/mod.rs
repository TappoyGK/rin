//! コメント・隣接関係を保持する構文木の構築と幅計算。
mod render;
use super::{
    WIDTH,
    syntax::{Plan, Style, Syntax},
};
use crate::lexer::{Token, TokenKind};
pub(super) use render::Output;
use std::ops::Range;

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
    flat: Option<usize>,
}
pub(super) struct Cst<'a> {
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
    pub(super) fn build(source: &'a str, syntax: &Syntax<'a>) -> Self {
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
            Kind::Atom(text) if !text.contains(['\n', '\r']) && text.chars().count() <= WIDTH => {
                Some(text.chars().count())
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
        if matches!(
            previous,
            TokenKind::Dot
                | TokenKind::Quote
                | TokenKind::Quasiquote
                | TokenKind::Unquote
                | TokenKind::UnquoteSplicing
        ) || matches!(next.kind, TokenKind::Dot | TokenKind::Colon)
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

    fn flat_items(&self, items: &[usize], group: Option<(char, char, Style)>) -> Option<usize> {
        let mut width = group.map_or(0, |(open, _, _)| {
            2 + if open == '{' && !items.is_empty() {
                2
            } else {
                0
            }
        });
        for (i, &id) in items.iter().enumerate() {
            let node = &self.nodes[id];
            if node.blank {
                return None;
            }
            width += node.flat?;
            if i > 0 {
                width += usize::from(self.space(items[i - 1], id));
            }
            if width > WIDTH {
                return None;
            }
        }
        (width <= WIDTH).then_some(width)
    }
}
