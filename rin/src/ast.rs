//! 作業領域 (`WorkArena`) 上に構築する抽象構文木（AST）とパターン表現を定義する。
//!
//! すべてのノードは作業領域の巻き戻しで一括解放されるため、`Copy` かつ `Drop` 不要となる。

use crate::error::Span;
use crate::mem::{RawStr, RawVec, WorkBox};

#[derive(Debug, Clone, Copy)]
pub struct Expr {
    pub span: Span,
    pub kind: ExprKind,
}

impl Expr {
    pub fn new(span: Span, kind: ExprKind) -> Self {
        Self { span, kind }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StructExpr {
    pub name: RawStr,
    pub fields: RawVec<(RawStr, Expr)>,
}

#[derive(Debug, Clone, Copy)]
pub struct VariantExpr {
    pub type_name: RawStr,
    pub variant: RawStr,
    pub data: Option<WorkBox<Expr>>,
}

#[derive(Debug, Clone, Copy)]
pub struct DefnExpr {
    pub name: RawStr,
    pub params: RawVec<RawStr>,
    pub body: RawVec<Expr>,
}

#[derive(Debug, Clone, Copy)]
pub struct FnExpr {
    pub params: RawVec<RawStr>,
    pub body: RawVec<Expr>,
}

#[derive(Debug, Clone, Copy)]
pub struct LetExpr {
    pub bindings: RawVec<(RawStr, Expr)>,
    pub body: RawVec<Expr>,
}

#[derive(Debug, Clone, Copy)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Str(RawStr),
    None,
    Ident(RawStr),
    List(RawVec<Expr>),
    Tuple(RawVec<Expr>),
    Struct(WorkBox<StructExpr>),
    Variant(WorkBox<VariantExpr>),
    Some(WorkBox<Expr>),
    Ok(WorkBox<Expr>),
    Err(WorkBox<Expr>),
    Field {
        target: WorkBox<Expr>,
        field: RawStr,
    },
    Index {
        target: WorkBox<Expr>,
        index: WorkBox<Expr>,
    },
    With {
        target: WorkBox<Expr>,
        fields: RawVec<(RawStr, Expr)>,
    },
    Def {
        name: RawStr,
        value: WorkBox<Expr>,
    },
    Import {
        name: RawStr,
    },
    Export {
        names: RawVec<RawStr>,
    },
    Defn(WorkBox<DefnExpr>),
    Fn(WorkBox<FnExpr>),
    Let(WorkBox<LetExpr>),
    If {
        cond: WorkBox<Expr>,
        then_branch: WorkBox<Expr>,
        else_branch: WorkBox<Expr>,
    },
    When {
        cond: WorkBox<Expr>,
        body: RawVec<Expr>,
    },
    Match {
        scrutinee: WorkBox<Expr>,
        arms: RawVec<MatchArm>,
    },
    Call {
        callee: WorkBox<Expr>,
        args: RawVec<Expr>,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct MatchArm {
    pub span: Span,
    pub pattern: Pattern,
    pub body: RawVec<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    Wildcard,
    Int(i64),
    Bool(bool),
    Char(char),
    Str(RawStr),
    None,
    Some(Option<RawStr>),
    Ok(Option<RawStr>),
    Err(Option<RawStr>),
    Variant {
        type_name: RawStr,
        variant: RawStr,
        binding: Option<RawStr>,
    },
    Var(RawStr),
}

/// バイトコード側が一時的に所有するパターン（ステップ 4 で永続領域 `Pattern` へ統合予定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OwnedPattern {
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

impl Pattern {
    pub(crate) fn to_owned_pattern(self) -> OwnedPattern {
        match self {
            Pattern::Wildcard => OwnedPattern::Wildcard,
            Pattern::Int(n) => OwnedPattern::Int(n),
            Pattern::Bool(b) => OwnedPattern::Bool(b),
            Pattern::Char(c) => OwnedPattern::Char(c),
            Pattern::Str(s) => OwnedPattern::Str(s.as_str().to_owned()),
            Pattern::None => OwnedPattern::None,
            Pattern::Some(b) => OwnedPattern::Some(b.map(|s| s.as_str().to_owned())),
            Pattern::Ok(b) => OwnedPattern::Ok(b.map(|s| s.as_str().to_owned())),
            Pattern::Err(b) => OwnedPattern::Err(b.map(|s| s.as_str().to_owned())),
            Pattern::Variant {
                type_name,
                variant,
                binding,
            } => OwnedPattern::Variant {
                type_name: type_name.as_str().to_owned(),
                variant: variant.as_str().to_owned(),
                binding: binding.map(|s| s.as_str().to_owned()),
            },
            Pattern::Var(s) => OwnedPattern::Var(s.as_str().to_owned()),
        }
    }
}

pub(crate) fn children<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match &expr.kind {
        ExprKind::List(xs) | ExprKind::Tuple(xs) => out.extend(xs.as_slice()),
        ExprKind::Fn(f) => out.extend(f.body.as_slice()),
        ExprKind::Defn(d) => out.extend(d.body.as_slice()),
        ExprKind::Struct(s) => out.extend(s.fields.as_slice().iter().map(|(_, e)| e)),
        ExprKind::Some(x)
        | ExprKind::Ok(x)
        | ExprKind::Err(x)
        | ExprKind::Def { value: x, .. }
        | ExprKind::Field { target: x, .. } => out.push(x.as_ref()),
        ExprKind::Variant(v) => {
            if let Some(d) = &v.data {
                out.push(d.as_ref());
            }
        }
        ExprKind::Index { target, index } => out.extend([target.as_ref(), index.as_ref()]),
        ExprKind::With { target, fields } => {
            out.push(target.as_ref());
            out.extend(fields.as_slice().iter().map(|(_, e)| e));
        }
        ExprKind::Let(l) => {
            out.extend(l.bindings.as_slice().iter().map(|(_, e)| e));
            out.extend(l.body.as_slice());
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => out.extend([cond.as_ref(), then_branch.as_ref(), else_branch.as_ref()]),
        ExprKind::When { cond, body } => {
            out.push(cond.as_ref());
            out.extend(body.as_slice());
        }
        ExprKind::Match { scrutinee, arms } => {
            out.push(scrutinee.as_ref());
            for arm in arms.as_slice() {
                out.extend(arm.body.as_slice());
            }
        }
        ExprKind::Call { callee, args } => {
            out.push(callee.as_ref());
            out.extend(args.as_slice());
        }
        _ => {}
    }
}
