use crate::ast::{Expr, ExprKind, children};
use crate::lexer::{Token, TokenKind};
use std::collections::BTreeMap;
use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub(super) enum Style {
    Call,
    Body(usize),
    If,
    Data,
    Tuple,
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
            children(expr, &mut work);
        }
        this
    }
    fn start(&self, e: &Expr) -> usize {
        self.positions[&(e.span.line, e.span.col)]
    }
    fn end(&self, e: &Expr) -> usize {
        let start = self.start(e);
        match &e.kind {
            ExprKind::Field { target, .. } => self.end(target) + 2,
            ExprKind::Index { target, .. } => self.ends[self.end(target)],
            ExprKind::Struct { .. } => self.ends[start + 1],
            ExprKind::Some(_) | ExprKind::Ok(_) | ExprKind::Err(_) => self.ends[start + 1],
            ExprKind::Variant { data, .. } => {
                if data.is_some() {
                    self.ends[start + 3]
                } else {
                    start + 3
                }
            }
            _ if self.ends[start] != 0 => self.ends[start],
            _ => start + 1,
        }
    }
    fn range(&self, e: &Expr) -> Range<usize> {
        self.start(e)..self.end(e)
    }
    fn add(&mut self, start: usize, style: Style, parts: Vec<Range<usize>>) {
        self.plans.insert(start, Plan { style, parts });
    }
    fn fields(&mut self, start: usize, fields: &[(String, Expr)]) {
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
            ExprKind::List(xs) | ExprKind::Tuple(xs) => self.add(
                start,
                if matches!(e.kind, ExprKind::Tuple(_)) {
                    Style::Tuple
                } else {
                    Style::Data
                },
                xs.iter().map(|e| self.range(e)).collect(),
            ),
            ExprKind::Call { callee, args } => {
                let style = if matches!(&callee.kind, ExprKind::Ident(n) if n == "deftest") {
                    Style::Body(2)
                } else {
                    Style::Call
                };
                self.add(
                    start,
                    style,
                    std::iter::once(callee.as_ref())
                        .chain(args)
                        .map(|e| self.range(e))
                        .collect(),
                );
            }
            ExprKind::Def { value, .. } => self.add(start, Style::Call, vec![self.range(value)]),
            ExprKind::Defn { body, .. } => {
                self.add(start + 3, Style::Params, vec![]);
                self.add(
                    start,
                    Style::Body(3),
                    body.iter().map(|e| self.range(e)).collect(),
                );
            }
            ExprKind::Fn { body, .. } => {
                self.add(start + 2, Style::Params, vec![]);
                self.add(
                    start,
                    Style::Body(2),
                    body.iter().map(|e| self.range(e)).collect(),
                );
            }
            ExprKind::Let { bindings, body } => {
                self.add(
                    start + 2,
                    Style::Bindings,
                    bindings
                        .iter()
                        .map(|(_, e)| self.start(e) - 1..self.end(e))
                        .collect(),
                );
                self.add(
                    start,
                    Style::Body(2),
                    body.iter().map(|e| self.range(e)).collect(),
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
                    .chain(body)
                    .map(|e| self.range(e))
                    .collect(),
            ),
            ExprKind::Match { scrutinee, arms } => {
                let parts = std::iter::once(self.range(scrutinee))
                    .chain(arms.iter().map(|a| {
                        let i = self.positions[&(a.span.line, a.span.col)];
                        i..self.ends[i]
                    }))
                    .collect();
                self.add(start, Style::Body(2), parts);
                for arm in arms {
                    let i = self.positions[&(arm.span.line, arm.span.col)];
                    let mut parts = Vec::with_capacity(arm.body.len() + 1);
                    parts.push(i + 1..self.start(&arm.body[0]));
                    parts.extend(arm.body.iter().map(|e| self.range(e)));
                    self.add(i, Style::Arm, parts);
                }
            }
            ExprKind::Struct { fields, .. } => self.fields(start + 1, fields),
            ExprKind::With { target, fields } => {
                let brace = self.end(target);
                self.fields(brace, fields);
                self.add(
                    start,
                    Style::Call,
                    vec![self.range(target), brace..self.ends[brace]],
                );
            }
            ExprKind::Some(x) | ExprKind::Ok(x) | ExprKind::Err(x) => {
                self.add(start + 1, Style::Call, vec![self.range(x)])
            }
            ExprKind::Variant { data: Some(x), .. } => {
                self.add(start + 3, Style::Call, vec![self.range(x)])
            }
            ExprKind::Index { target, index } => {
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
            (
                ExprKind::Struct {
                    name: a,
                    fields: af,
                },
                ExprKind::Struct {
                    name: b,
                    fields: bf,
                },
            ) => a == b && af.iter().map(|(n, _)| n).eq(bf.iter().map(|(n, _)| n)),
            (ExprKind::With { fields: a, .. }, ExprKind::With { fields: b, .. }) => {
                a.iter().map(|(n, _)| n).eq(b.iter().map(|(n, _)| n))
            }
            (
                ExprKind::Variant {
                    type_name: a,
                    variant: av,
                    ..
                },
                ExprKind::Variant {
                    type_name: b,
                    variant: bv,
                    ..
                },
            ) => a == b && av == bv,
            (ExprKind::Field { field: a, .. }, ExprKind::Field { field: b, .. }) => a == b,
            (ExprKind::Def { name: a, .. }, ExprKind::Def { name: b, .. })
            | (ExprKind::Import { name: a }, ExprKind::Import { name: b }) => a == b,
            (ExprKind::Export { names: a }, ExprKind::Export { names: b }) => a == b,
            (
                ExprKind::Defn {
                    name: a,
                    params: ap,
                    ..
                },
                ExprKind::Defn {
                    name: b,
                    params: bp,
                    ..
                },
            ) => a == b && ap == bp,
            (ExprKind::Fn { params: a, .. }, ExprKind::Fn { params: b, .. }) => a == b,
            (ExprKind::Let { bindings: a, .. }, ExprKind::Let { bindings: b, .. }) => {
                a.iter().map(|(n, _)| n).eq(b.iter().map(|(n, _)| n))
            }
            (ExprKind::Match { arms: a, .. }, ExprKind::Match { arms: b, .. }) => {
                a.len() == b.len()
                    && a.iter()
                        .zip(b)
                        .all(|(a, b)| a.pattern == b.pattern && a.body.len() == b.body.len())
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

#[cfg(test)]
mod tests {
    use super::equivalent;
    use crate::parser::parse_tests;

    #[test]
    fn preservation_check_rejects_semantic_changes() {
        for (before, after) in [
            ("1", "2"),
            ("0.0", "-0.0"),
            ("[1 2]", "(1, 2)"),
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
                !equivalent(&parse_tests(before).unwrap(), &parse_tests(after).unwrap()),
                "{before} -> {after}"
            );
        }
        assert!(equivalent(
            &parse_tests("[nan 1]").unwrap(),
            &parse_tests("\n[ nan,\n 1 ]\n").unwrap()
        ));
    }
}
