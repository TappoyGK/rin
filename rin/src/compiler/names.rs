//! 診断表が必要になったときだけ、AST から容量の上限を求める。
//! 幅の広いリストでも作業スタックに兄弟を一括で積まず、深さだけに比例させる。
use crate::ast::{Expr, ExprKind, MatchArm};
use crate::bytecode::NameTable;

pub(super) fn prepare(exprs: &[Expr]) -> NameTable {
    enum Scan<'a> {
        Expr(&'a Expr),
        Exprs(&'a [Expr]),
        Fields(&'a [(String, Expr)], bool), // true は let の束縛
        Arms(&'a [MatchArm]),
    }
    let mut bindings = 0;
    let mut bytes = 0;
    let mut sites = 0;
    let mut work = Vec::with_capacity(32);
    work.push(Scan::Exprs(exprs));
    while let Some(task) = work.pop() {
        match task {
            Scan::Exprs(items) => {
                if let Some((first, rest)) = items.split_first() {
                    if !rest.is_empty() {
                        work.push(Scan::Exprs(rest));
                    }
                    work.push(Scan::Expr(first));
                }
            }
            Scan::Fields(items, binding) => {
                if let Some(((name, first), rest)) = items.split_first() {
                    if binding {
                        bindings += 1;
                        bytes += name.len();
                    }
                    if !rest.is_empty() {
                        work.push(Scan::Fields(rest, binding));
                    }
                    work.push(Scan::Expr(first));
                }
            }
            Scan::Arms(arms) => {
                if let Some((first, rest)) = arms.split_first() {
                    if let Some(name) = super::pattern_binding(&first.pattern) {
                        bindings += 1;
                        bytes += name.len();
                    }
                    if !rest.is_empty() {
                        work.push(Scan::Arms(rest));
                    }
                    work.push(Scan::Exprs(&first.body));
                }
            }
            Scan::Expr(expr) => match &expr.kind {
                ExprKind::Ident(name) => sites += usize::from(name.len() >= 3),
                ExprKind::List(items) | ExprKind::Tuple(items) => work.push(Scan::Exprs(items)),
                ExprKind::Struct { fields, .. } => work.push(Scan::Fields(fields, false)),
                ExprKind::Variant {
                    data: Some(value), ..
                }
                | ExprKind::Some(value)
                | ExprKind::Ok(value)
                | ExprKind::Err(value)
                | ExprKind::Field { target: value, .. }
                | ExprKind::Def { value, .. } => work.push(Scan::Expr(value)),
                ExprKind::Index { target, index } => {
                    work.push(Scan::Expr(index));
                    work.push(Scan::Expr(target));
                }
                ExprKind::With { target, fields } => {
                    work.push(Scan::Fields(fields, false));
                    work.push(Scan::Expr(target));
                }
                ExprKind::Defn { params, body, .. } | ExprKind::Fn { params, body } => {
                    bindings += params.len();
                    bytes += params.iter().map(String::len).sum::<usize>();
                    work.push(Scan::Exprs(body));
                }
                ExprKind::Let { bindings, body } => {
                    work.push(Scan::Exprs(body));
                    work.push(Scan::Fields(bindings, true));
                }
                ExprKind::If {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    work.push(Scan::Expr(else_branch));
                    work.push(Scan::Expr(then_branch));
                    work.push(Scan::Expr(cond));
                }
                ExprKind::When { cond, body } => {
                    work.push(Scan::Exprs(body));
                    work.push(Scan::Expr(cond));
                }
                ExprKind::Match { scrutinee, arms } => {
                    work.push(Scan::Arms(arms));
                    work.push(Scan::Expr(scrutinee));
                }
                ExprKind::Call { callee, args } => {
                    work.push(Scan::Exprs(args));
                    work.push(Scan::Expr(callee));
                }
                ExprKind::Int(_)
                | ExprKind::Float(_)
                | ExprKind::Bool(_)
                | ExprKind::Char(_)
                | ExprKind::Str(_)
                | ExprKind::None
                | ExprKind::Import { .. }
                | ExprKind::Export { .. }
                | ExprKind::Variant { data: None, .. } => {}
            },
        }
    }
    drop(work);
    // 各バッファは 1 回だけ確保し、束縛数・関数数が増えても途中で再確保しない。
    NameTable {
        text: String::with_capacity(bytes),
        nodes: Vec::with_capacity(bindings),
        sites: Vec::with_capacity(sites),
    }
}
