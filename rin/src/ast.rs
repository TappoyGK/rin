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
        let mut work = Vec::new();
        take_expr_children(&mut self.kind, &mut work);
        while let Some(mut child) = work.pop() {
            take_expr_children(&mut child.kind, &mut work);
        }
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
        | ExprKind::Export { .. } => {}
        ExprKind::List(items) | ExprKind::Tuple(items) => out.extend(items),
        ExprKind::Struct { fields, .. } => {
            for (_, v) in fields {
                out.push(v);
            }
        }
        ExprKind::Variant { data: Some(d), .. }
        | ExprKind::Some(d)
        | ExprKind::Ok(d)
        | ExprKind::Err(d)
        | ExprKind::Field { target: d, .. }
        | ExprKind::Def { value: d, .. } => out.push(*d),
        ExprKind::Variant { data: None, .. } => {}
        ExprKind::Index { target, index } => {
            out.push(*target);
            out.push(*index);
        }
        ExprKind::With { target, fields } => {
            out.push(*target);
            for (_, v) in fields {
                out.push(v);
            }
        }
        ExprKind::Defn { body, .. } | ExprKind::Fn { body, .. } => out.extend(body),
        ExprKind::Let { bindings, body } => {
            for (_, v) in bindings {
                out.push(v);
            }
            out.extend(body);
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            out.push(*cond);
            out.push(*then_branch);
            out.push(*else_branch);
        }
        ExprKind::When { cond, body } => {
            out.push(*cond);
            out.extend(body);
        }
        ExprKind::Match { scrutinee, arms } => {
            out.push(*scrutinee);
            for arm in arms {
                out.extend(arm.body);
            }
        }
        ExprKind::Call { callee, args } => {
            out.push(*callee);
            out.extend(args);
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
