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

const SHALLOW_EXPR_DEPTH: usize = 32;

impl Drop for Expr {
    #[inline]
    fn drop(&mut self) {
        self.drop_shallow(0);
    }
}

impl Expr {
    fn drop_shallow(&mut self, depth: usize) {
        if !has_expr_children(&self.kind) {
            return;
        }
        if depth >= SHALLOW_EXPR_DEPTH {
            self.drop_iter();
            return;
        }
        match std::mem::replace(&mut self.kind, ExprKind::None) {
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
            ExprKind::List(mut items) | ExprKind::Tuple(mut items) => {
                for item in &mut items {
                    item.drop_shallow(depth + 1);
                }
            }
            ExprKind::Struct { mut fields, .. } => {
                for (_, v) in &mut fields {
                    v.drop_shallow(depth + 1);
                }
            }
            ExprKind::Variant {
                data: Some(mut d), ..
            }
            | ExprKind::Some(mut d)
            | ExprKind::Ok(mut d)
            | ExprKind::Err(mut d)
            | ExprKind::Field { target: mut d, .. }
            | ExprKind::Def { value: mut d, .. } => d.drop_shallow(depth + 1),
            ExprKind::Index {
                mut target,
                mut index,
            } => {
                target.drop_shallow(depth + 1);
                index.drop_shallow(depth + 1);
            }
            ExprKind::With {
                mut target,
                mut fields,
            } => {
                target.drop_shallow(depth + 1);
                for (_, v) in &mut fields {
                    v.drop_shallow(depth + 1);
                }
            }
            ExprKind::Defn { mut body, .. } | ExprKind::Fn { mut body, .. } => {
                for item in &mut body {
                    item.drop_shallow(depth + 1);
                }
            }
            ExprKind::Let {
                mut bindings,
                mut body,
            } => {
                for (_, v) in &mut bindings {
                    v.drop_shallow(depth + 1);
                }
                for item in &mut body {
                    item.drop_shallow(depth + 1);
                }
            }
            ExprKind::If {
                mut cond,
                mut then_branch,
                mut else_branch,
            } => {
                cond.drop_shallow(depth + 1);
                then_branch.drop_shallow(depth + 1);
                else_branch.drop_shallow(depth + 1);
            }
            ExprKind::When { mut cond, mut body } => {
                cond.drop_shallow(depth + 1);
                for item in &mut body {
                    item.drop_shallow(depth + 1);
                }
            }
            ExprKind::Match {
                mut scrutinee,
                mut arms,
            } => {
                scrutinee.drop_shallow(depth + 1);
                for arm in &mut arms {
                    for item in &mut arm.body {
                        item.drop_shallow(depth + 1);
                    }
                }
            }
            ExprKind::Call {
                mut callee,
                mut args,
            } => {
                callee.drop_shallow(depth + 1);
                for arg in &mut args {
                    arg.drop_shallow(depth + 1);
                }
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn drop_iter(&mut self) {
        let mut next = None;
        let mut work = Vec::new();
        take_expr_children(&mut self.kind, &mut next, &mut work);
        loop {
            let mut cur = match next.take() {
                Some(c) => c,
                None => match work.pop() {
                    Some(c) => c,
                    None => break,
                },
            };
            take_expr_children(&mut cur.kind, &mut next, &mut work);
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
fn push_if_non_leaf(expr: Expr, next: &mut Option<Expr>, work: &mut Vec<Expr>) {
    if has_expr_children(&expr.kind) {
        if next.is_none() {
            *next = Some(expr);
        } else {
            work.push(expr);
        }
    }
}

fn take_expr_children(kind: &mut ExprKind, next: &mut Option<Expr>, work: &mut Vec<Expr>) {
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
                push_if_non_leaf(item, next, work);
            }
        }
        ExprKind::Struct { fields, .. } => {
            for (_, v) in fields {
                push_if_non_leaf(v, next, work);
            }
        }
        ExprKind::Variant { data: Some(d), .. }
        | ExprKind::Some(d)
        | ExprKind::Ok(d)
        | ExprKind::Err(d)
        | ExprKind::Field { target: d, .. }
        | ExprKind::Def { value: d, .. } => push_if_non_leaf(*d, next, work),
        ExprKind::Index { target, index } => {
            push_if_non_leaf(*target, next, work);
            push_if_non_leaf(*index, next, work);
        }
        ExprKind::With { target, fields } => {
            push_if_non_leaf(*target, next, work);
            for (_, v) in fields {
                push_if_non_leaf(v, next, work);
            }
        }
        ExprKind::Defn { body, .. } | ExprKind::Fn { body, .. } => {
            for item in body {
                push_if_non_leaf(item, next, work);
            }
        }
        ExprKind::Let { bindings, body } => {
            for (_, v) in bindings {
                push_if_non_leaf(v, next, work);
            }
            for item in body {
                push_if_non_leaf(item, next, work);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            push_if_non_leaf(*cond, next, work);
            push_if_non_leaf(*then_branch, next, work);
            push_if_non_leaf(*else_branch, next, work);
        }
        ExprKind::When { cond, body } => {
            push_if_non_leaf(*cond, next, work);
            for item in body {
                push_if_non_leaf(item, next, work);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            push_if_non_leaf(*scrutinee, next, work);
            for arm in arms {
                for item in arm.body {
                    push_if_non_leaf(item, next, work);
                }
            }
        }
        ExprKind::Call { callee, args } => {
            push_if_non_leaf(*callee, next, work);
            for arg in args {
                push_if_non_leaf(arg, next, work);
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
