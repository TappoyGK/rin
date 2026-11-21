//! 構文解析で生成する抽象構文木（AST）とパターン表現を定義する。

use crate::error::Span;

#[derive(Debug, Clone)]
pub struct Expr {
    pub span: Span,
    pub kind: ExprKind,
}

impl Expr {
    pub fn new(span: Span, kind: ExprKind) -> Self {
        Self { span, kind }
    }
}

impl Drop for Expr {
    fn drop(&mut self) {
        if !has_expr_children(&self.kind) {
            return;
        }
        let mut work = Vec::with_capacity(8);
        take_expr_children(&mut self.kind, &mut work);
        while let Some(mut child) = work.pop() {
            take_expr_children(&mut child.kind, &mut work);
        }
    }
}

#[inline(always)]
fn has_expr_children(kind: &ExprKind) -> bool {
    !matches!(
        kind,
        ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Ident(_)
            | ExprKind::Import { .. }
            | ExprKind::Export { .. }
            | ExprKind::Variant { data: None, .. }
    )
}

#[inline(always)]
fn push_if_non_leaf(expr: Expr, out: &mut Vec<Expr>) {
    if has_expr_children(&expr.kind) {
        out.push(expr);
    }
}

fn take_expr_children(kind: &mut ExprKind, out: &mut Vec<Expr>) {
    match std::mem::replace(kind, ExprKind::None) {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Char(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Ident(_)
        | ExprKind::Import { .. }
        | ExprKind::Export { .. }
        | ExprKind::Variant { data: None, .. } => {}
        ExprKind::List(items) | ExprKind::Tuple(items) => {
            for item in items {
                push_if_non_leaf(item, out);
            }
        }
        ExprKind::Struct { fields, .. } => {
            for (_, v) in fields {
                push_if_non_leaf(v, out);
            }
        }
        ExprKind::Variant { data: Some(d), .. }
        | ExprKind::Some(d)
        | ExprKind::Ok(d)
        | ExprKind::Err(d)
        | ExprKind::Field { target: d, .. }
        | ExprKind::Def { value: d, .. } => push_if_non_leaf(*d, out),
        ExprKind::Index { target, index } => {
            push_if_non_leaf(*target, out);
            push_if_non_leaf(*index, out);
        }
        ExprKind::With { target, fields } => {
            push_if_non_leaf(*target, out);
            for (_, v) in fields {
                push_if_non_leaf(v, out);
            }
        }
        ExprKind::Defn { body, .. } | ExprKind::Fn { body, .. } => {
            for item in body {
                push_if_non_leaf(item, out);
            }
        }
        ExprKind::Let { bindings, body } => {
            for (_, v) in bindings {
                push_if_non_leaf(v, out);
            }
            for item in body {
                push_if_non_leaf(item, out);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            push_if_non_leaf(*cond, out);
            push_if_non_leaf(*then_branch, out);
            push_if_non_leaf(*else_branch, out);
        }
        ExprKind::When { cond, body } => {
            push_if_non_leaf(*cond, out);
            for item in body {
                push_if_non_leaf(item, out);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            push_if_non_leaf(*scrutinee, out);
            for arm in arms {
                for item in arm.body {
                    push_if_non_leaf(item, out);
                }
            }
        }
        ExprKind::Call { callee, args } => {
            push_if_non_leaf(*callee, out);
            for arg in args {
                push_if_non_leaf(arg, out);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Str(String),
    None,
    Ident(String),
    List(Vec<Expr>),
    Tuple(Vec<Expr>),
    Struct {
        name: String,
        fields: Vec<(String, Expr)>,
    },
    Variant {
        type_name: String,
        variant: String,
        data: Option<Box<Expr>>,
    },
    Some(Box<Expr>),
    Ok(Box<Expr>),
    Err(Box<Expr>),
    Field {
        target: Box<Expr>,
        field: String,
    },
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },
    With {
        target: Box<Expr>,
        fields: Vec<(String, Expr)>,
    },
    Def {
        name: String,
        value: Box<Expr>,
    },
    Import {
        name: String,
    },
    Export {
        names: Vec<String>,
    },
    Defn {
        name: String,
        params: Vec<String>,
        body: Vec<Expr>,
    },
    Fn {
        params: Vec<String>,
        body: Vec<Expr>,
    },
    Let {
        bindings: Vec<(String, Expr)>,
        body: Vec<Expr>,
    },
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    When {
        cond: Box<Expr>,
        body: Vec<Expr>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub span: Span,
    pub pattern: Pattern,
    pub body: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,
    Int(i64),
    Bool(bool),
    Char(char),
    Str(String),
    None,
    Some(Option<String>),
    Ok(Option<String>),
    Err(Option<String>),
    Variant {
        type_name: String,
        variant: String,
        binding: Option<String>,
    },
    Var(String),
}
