//! 深いスキーマの解放。
use super::{
    Schema,
    types::{TypeExpr, has_type_children},
};

#[inline(always)]
fn push_type_if_non_leaf(ty: TypeExpr, next: &mut Option<TypeExpr>, work: &mut Vec<TypeExpr>) {
    if has_type_children(&ty) {
        if next.is_none() {
            *next = Some(ty);
        } else {
            work.push(ty);
        }
    }
}

#[cold]
#[inline(never)]
pub(super) fn drop_deep_type(ty: &mut TypeExpr) {
    if !has_type_children(ty) {
        return;
    }
    let mut next = None;
    let mut work = Vec::new();
    take_type_children(ty, &mut next, &mut work);
    loop {
        let mut cur = match next.take() {
            Some(c) => c,
            None => match work.pop() {
                Some(c) => c,
                None => break,
            },
        };
        take_type_children(&mut cur, &mut next, &mut work);
    }
}

fn take_type_children(ty: &mut TypeExpr, next: &mut Option<TypeExpr>, work: &mut Vec<TypeExpr>) {
    match ty {
        TypeExpr::Any
        | TypeExpr::Void
        | TypeExpr::I64
        | TypeExpr::F64
        | TypeExpr::Bool
        | TypeExpr::Char
        | TypeExpr::String
        | TypeExpr::Ref(_) => {}
        TypeExpr::List(t) | TypeExpr::Array(t) | TypeExpr::Option(t) => {
            push_type_if_non_leaf(std::mem::replace(&mut **t, TypeExpr::Any), next, work);
        }
        TypeExpr::Result(a, b) => {
            push_type_if_non_leaf(std::mem::replace(&mut **a, TypeExpr::Any), next, work);
            push_type_if_non_leaf(std::mem::replace(&mut **b, TypeExpr::Any), next, work);
        }
        TypeExpr::Tuple(elems) => {
            for t in std::mem::take(elems) {
                push_type_if_non_leaf(t, next, work);
            }
        }
        TypeExpr::Struct { fields, .. } => {
            for (_, t) in std::mem::take(fields) {
                push_type_if_non_leaf(t, next, work);
            }
        }
        TypeExpr::Variant { cases, .. } => {
            for (_, opt) in std::mem::take(cases) {
                if let Some(t) = opt {
                    push_type_if_non_leaf(t, next, work);
                }
            }
        }
    }
}

impl Drop for Schema {
    #[inline]
    fn drop(&mut self) {
        if self.has_deep {
            self.drop_deep();
        }
    }
}

impl Schema {
    #[cold]
    #[inline(never)]
    fn drop_deep(&mut self) {
        drop_deep_type(&mut self.root);
        for (_, mut ty) in std::mem::take(&mut self.defs) {
            drop_deep_type(&mut ty);
        }
    }
}
