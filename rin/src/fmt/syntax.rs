use crate::ast::{Datum, DatumKind, Expr, ExprKind, children};
use crate::lexer::{Token, TokenKind};
use std::collections::BTreeMap;
use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub(super) enum Style {
    Call,
    Body(usize),
    If,
    Data,
    Fields,
    Bindings,
    Params,
    Arm,
    Root,
}
pub(super) struct Plan {
    pub style: Style,
    pub parts: Vec<Range<usize>>,
}
pub(super) struct Syntax<'a> {
    pub tokens: &'a [(Token, Range<usize>)],
    pub ends: Vec<usize>,
    positions: BTreeMap<(usize, usize), usize>,
    pub plans: BTreeMap<usize, Plan>,
}
impl<'a> Syntax<'a> {
    pub fn new(tokens: &'a [(Token, Range<usize>)], exprs: &[Expr]) -> Self {
        let mut ends = vec![0; tokens.len()];
        let mut stack = Vec::new();
        for (i, (token, _)) in tokens.iter().enumerate() {
            match token.kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => stack.push(i),
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    ends[stack.pop().unwrap()] = i + 1;
                }
                _ => {}
            }
        }
        let positions = tokens
            .iter()
            .enumerate()
            .map(|(i, (t, _))| ((t.span.line, t.span.col), i))
            .collect();
        let mut this = Self {
            tokens,
            ends,
            positions,
            plans: BTreeMap::new(),
        };
        let root = Plan {
            style: Style::Root,
            parts: exprs.iter().map(|e| this.range(e)).collect(),
        };
        this.plans.insert(tokens.len(), root);
        let mut work: Vec<_> = exprs.iter().collect();
        while let Some(expr) = work.pop() {
            this.plan(expr);
            if !matches!(expr.kind, ExprKind::Template(_)) {
                children(expr, &mut work);
            }
        }
        this
    }
    fn start(&self, e: &Expr) -> usize {
        self.positions[&(e.span.line, e.span.col)]
    }
    fn end(&self, e: &Expr) -> usize {
        let start = self.start(e);
        if self.ends[start] != 0 {
            return self.ends[start];
        }
        match &e.kind {
            ExprKind::Field { target, .. } => self.end(target) + 2,
            ExprKind::Index { target, .. } => self.ends[self.end(target)],
            ExprKind::Struct(_) => self.ends[start + 1],
            ExprKind::Some(_) | ExprKind::Ok(_) | ExprKind::Err(_) => self.ends[start + 1],
            ExprKind::Variant(v) => {
                if v.data.is_some() {
                    self.ends[start + 3]
                } else {
                    start + 3
                }
            }
            ExprKind::Quote(_) | ExprKind::Quasiquote(_) => self.datum_token_end(start),
            _ => start + 1,
        }
    }
    fn datum_token_end(&self, mut pos: usize) -> usize {
        while pos < self.tokens.len()
            && matches!(
                self.tokens[pos].0.kind,
                TokenKind::Quote
                    | TokenKind::Quasiquote
                    | TokenKind::Unquote
                    | TokenKind::UnquoteSplicing
            )
        {
            pos += 1;
        }
        if pos >= self.tokens.len() {
            return pos;
        }
        let mut end = if self.ends[pos] != 0 {
            self.ends[pos]
        } else if let TokenKind::Ident(name) = &self.tokens[pos].0.kind {
            let is_upper = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
            if (matches!(name.as_str(), "some" | "ok" | "err")
                && self
                    .tokens
                    .get(pos + 1)
                    .is_some_and(|(t, _)| t.kind == TokenKind::LParen && t.attached))
                || (is_upper
                    && self
                        .tokens
                        .get(pos + 1)
                        .is_some_and(|(t, _)| t.kind == TokenKind::LBrace))
            {
                self.ends[pos + 1]
            } else if is_upper
                && self
                    .tokens
                    .get(pos + 1)
                    .is_some_and(|(t, _)| t.kind == TokenKind::Dot)
            {
                if self
                    .tokens
                    .get(pos + 3)
                    .is_some_and(|(t, _)| t.kind == TokenKind::LParen && t.attached)
                {
                    self.ends[pos + 3]
                } else {
                    pos + 3
                }
            } else {
                pos + 1
            }
        } else {
            pos + 1
        };
        while end < self.tokens.len() {
            let tok = &self.tokens[end].0;
            if tok.kind == TokenKind::Dot {
                end += 2;
            } else if tok.kind == TokenKind::LBracket && tok.attached {
                end = self.ends[end];
            } else {
                break;
            }
        }
        end
    }
    fn range(&self, e: &Expr) -> Range<usize> {
        self.start(e)..self.end(e)
    }
    fn add(&mut self, start: usize, style: Style, parts: Vec<Range<usize>>) {
        self.plans.insert(start, Plan { style, parts });
    }
    fn fields(&mut self, start: usize, fields: &[(crate::mem::RawStr, Expr)]) {
        self.add(
            start,
            Style::Fields,
            fields
                .iter()
                .map(|(_, e)| self.start(e) - 2..self.end(e))
                .collect(),
        );
    }
    fn plan(&mut self, e: &Expr) {
        let start = self.start(e);
        match &e.kind {
            ExprKind::List(xs) => self.add(
                start,
                Style::Data,
                xs.as_slice().iter().map(|e| self.range(e)).collect(),
            ),
            ExprKind::Tuple(xs) => self.add(
                start,
                Style::Call,
                xs.as_slice().iter().map(|e| self.range(e)).collect(),
            ),
            ExprKind::Call { callee, args } => {
                let style = if matches!(&callee.kind, ExprKind::Ident(n) if n.as_str() == "deftest")
                {
                    Style::Body(2)
                } else {
                    Style::Call
                };
                self.add(
                    start,
                    style,
                    std::iter::once(callee.as_ref())
                        .chain(args.as_slice())
                        .map(|e| self.range(e))
                        .collect(),
                );
            }
            ExprKind::Def { value, .. } => self.add(start, Style::Call, vec![self.range(value)]),
            ExprKind::Defn(d) => {
                self.add(start + 3, Style::Params, vec![]);
                self.add(
                    start,
                    Style::Body(3),
                    d.body.as_slice().iter().map(|e| self.range(e)).collect(),
                );
            }
            ExprKind::Fn(f) => {
                self.add(start + 2, Style::Params, vec![]);
                self.add(
                    start,
                    Style::Body(2),
                    f.body.as_slice().iter().map(|e| self.range(e)).collect(),
                );
            }
            ExprKind::Let(l) => {
                self.add(
                    start + 2,
                    Style::Bindings,
                    l.bindings
                        .as_slice()
                        .iter()
                        .map(|(_, e)| self.start(e) - 1..self.end(e))
                        .collect(),
                );
                self.add(
                    start,
                    Style::Body(2),
                    l.body.as_slice().iter().map(|e| self.range(e)).collect(),
                );
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.add(
                start,
                Style::If,
                [cond.as_ref(), then_branch.as_ref(), else_branch.as_ref()]
                    .iter()
                    .map(|e| self.range(e))
                    .collect(),
            ),
            ExprKind::When { cond, body } => self.add(
                start,
                Style::Body(2),
                std::iter::once(cond.as_ref())
                    .chain(body.as_slice())
                    .map(|e| self.range(e))
                    .collect(),
            ),
            ExprKind::Match { scrutinee, arms } => {
                let arms = arms.as_slice();
                let parts = std::iter::once(self.range(scrutinee))
                    .chain(arms.iter().map(|a| {
                        let i = self.positions[&(a.span.line, a.span.col)];
                        i..self.ends[i]
                    }))
                    .collect();
                self.add(start, Style::Body(2), parts);
                for arm in arms {
                    let i = self.positions[&(arm.span.line, arm.span.col)];
                    let body = arm.body.as_slice();
                    let mut parts = Vec::with_capacity(body.len() + 1);
                    parts.push(i + 1..self.start(&body[0]));
                    parts.extend(body.iter().map(|e| self.range(e)));
                    self.add(i, Style::Arm, parts);
                }
            }
            ExprKind::Struct(s) if self.tokens[start].0.kind != TokenKind::LParen => {
                self.fields(start + 1, s.fields.as_slice())
            }
            ExprKind::With { target, fields } => {
                let brace = self.end(target);
                if self
                    .tokens
                    .get(brace)
                    .is_some_and(|(t, _)| t.kind == TokenKind::LBrace)
                {
                    self.fields(brace, fields.as_slice());
                    self.add(
                        start,
                        Style::Call,
                        vec![self.range(target), brace..self.ends[brace]],
                    );
                } else {
                    self.add(start, Style::Call, vec![self.range(target)]);
                }
            }
            ExprKind::Some(x) | ExprKind::Ok(x) | ExprKind::Err(x) => {
                let paren = if self.tokens[start].0.kind == TokenKind::LParen {
                    start
                } else {
                    start + 1
                };
                self.add(paren, Style::Call, vec![self.range(x)])
            }
            ExprKind::Variant(v)
                if v.data.is_some() && self.tokens[start].0.kind != TokenKind::LParen =>
            {
                self.add(
                    start + 3,
                    Style::Call,
                    vec![self.range(v.data.as_ref().unwrap())],
                )
            }
            ExprKind::Index { target, index } if self.tokens[start].0.kind != TokenKind::LParen => {
                self.add(self.end(target), Style::Call, vec![self.range(index)])
            }
            _ => {}
        }
    }
}

/// 位置を除いた構造を、再帰せず比較する。浮動小数点はNaNと負のゼロも保持する。
pub(super) fn equivalent(a: &[Expr], b: &[Expr]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut work: Vec<_> = a.iter().zip(b).collect();
    let mut left = Vec::new();
    let mut right = Vec::new();
    while let Some((a, b)) = work.pop() {
        if std::mem::discriminant(&a.kind) != std::mem::discriminant(&b.kind) {
            return false;
        }
        let equal = match (&a.kind, &b.kind) {
            (ExprKind::Int(a), ExprKind::Int(b)) => a == b,
            (ExprKind::Float(a), ExprKind::Float(b)) => a.to_bits() == b.to_bits(),
            (ExprKind::Bool(a), ExprKind::Bool(b)) => a == b,
            (ExprKind::Char(a), ExprKind::Char(b)) => a == b,
            (ExprKind::Str(a), ExprKind::Str(b)) | (ExprKind::Ident(a), ExprKind::Ident(b)) => {
                a == b
            }
            (ExprKind::Struct(a), ExprKind::Struct(b)) => {
                a.name == b.name
                    && a.fields.as_slice().iter().map(|(n, _)| n).eq(b
                        .fields
                        .as_slice()
                        .iter()
                        .map(|(n, _)| n))
            }
            (ExprKind::With { fields: a, .. }, ExprKind::With { fields: b, .. }) => a
                .as_slice()
                .iter()
                .map(|(n, _)| n)
                .eq(b.as_slice().iter().map(|(n, _)| n)),
            (ExprKind::Variant(a), ExprKind::Variant(b)) => {
                a.type_name == b.type_name && a.variant == b.variant
            }
            (ExprKind::Field { field: a, .. }, ExprKind::Field { field: b, .. }) => a == b,
            (ExprKind::Def { name: a, .. }, ExprKind::Def { name: b, .. })
            | (ExprKind::Import { name: a }, ExprKind::Import { name: b }) => a == b,
            (ExprKind::Export { names: a }, ExprKind::Export { names: b }) => {
                a.as_slice() == b.as_slice()
            }
            (ExprKind::Defn(a), ExprKind::Defn(b)) => {
                a.name == b.name && a.params.as_slice() == b.params.as_slice()
            }
            (ExprKind::Fn(a), ExprKind::Fn(b)) => a.params.as_slice() == b.params.as_slice(),
            (ExprKind::Let(a), ExprKind::Let(b)) => a
                .bindings
                .as_slice()
                .iter()
                .map(|(n, _)| n)
                .eq(b.bindings.as_slice().iter().map(|(n, _)| n)),
            (ExprKind::Match { arms: a, .. }, ExprKind::Match { arms: b, .. }) => {
                let a = a.as_slice();
                let b = b.as_slice();
                a.len() == b.len()
                    && a.iter()
                        .zip(b)
                        .all(|(a, b)| a.pattern == b.pattern && a.body.len() == b.body.len())
            }
            (ExprKind::Quote(a), ExprKind::Quote(b))
            | (ExprKind::Quasiquote(a), ExprKind::Quasiquote(b)) => {
                datum_equivalent(a.as_ref(), b.as_ref())
            }
            _ => true,
        };
        if !equal {
            return false;
        }
        left.clear();
        right.clear();
        children(a, &mut left);
        children(b, &mut right);
        if left.len() != right.len() {
            return false;
        }
        work.extend(left.iter().copied().zip(right.iter().copied()));
    }
    true
}

fn datum_equivalent(a: &Datum, b: &Datum) -> bool {
    let mut stack = vec![(a, b)];
    while let Some((a, b)) = stack.pop() {
        if std::mem::discriminant(&a.kind) != std::mem::discriminant(&b.kind) {
            return false;
        }
        match (&a.kind, &b.kind) {
            (DatumKind::Int(x), DatumKind::Int(y)) if x != y => return false,
            (DatumKind::Float(x), DatumKind::Float(y)) if x.to_bits() != y.to_bits() => {
                return false;
            }
            (DatumKind::Bool(x), DatumKind::Bool(y)) if x != y => return false,
            (DatumKind::Char(x), DatumKind::Char(y)) if x != y => return false,
            (DatumKind::Str(x), DatumKind::Str(y)) | (DatumKind::Ident(x), DatumKind::Ident(y))
                if x != y =>
            {
                return false;
            }
            (DatumKind::List(xs), DatumKind::List(ys)) => {
                let xs = xs.as_slice();
                let ys = ys.as_slice();
                if xs.len() != ys.len() {
                    return false;
                }
                stack.extend(xs.iter().zip(ys.iter()));
            }
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::equivalent;
    use crate::parser::parse_tests;

    #[test]
    fn preservation_check_rejects_semantic_changes() {
        for (before, after) in [
            ("1", "2"),
            ("0.0", "-0.0"),
            ("[1 2]", "(tuple 1 2)"),
            ("(f 1 2)", "(f 2 1)"),
            ("(def x 1)", "(def y 1)"),
            ("(fn (x) x)", "(fn (y) x)"),
            ("(let (x 1) x)", "(let (y 1) x)"),
            ("Foo {x: 1}", "Foo {y: 1}"),
            ("Foo {x: 1}", "Bar {x: 1}"),
            ("Kind.A(1)", "Kind.B(1)"),
            ("x.a", "x.b"),
            ("(export x)", "(export y)"),
            ("(match x (some(a) a) (_ 0))", "(match x (some(b) a) (_ 0))"),
            ("(match x (1 2 3) (_ 4))", "(match x (1 2) (_ 3 4))"),
        ] {
            assert!(
                !equivalent(
                    parse_tests(before).unwrap().exprs(),
                    parse_tests(after).unwrap().exprs()
                ),
                "{before} -> {after}"
            );
        }
        assert!(equivalent(
            parse_tests("[nan 1]").unwrap().exprs(),
            parse_tests("\n[ nan\n 1 ]\n").unwrap().exprs()
        ));
    }
}
