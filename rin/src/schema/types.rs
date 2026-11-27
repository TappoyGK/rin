/// スキーマの型式 AST。
#[derive(Debug, Clone)]
pub(crate) enum TypeExpr {
    Datum,
    Void,
    I64,
    F64,
    Bool,
    Char,
    Text,
    Ident,
    List(Box<TypeExpr>),
    Array(Box<TypeExpr>),
    Tuple(Vec<TypeExpr>),
    Option(Box<TypeExpr>),
    Result(Box<TypeExpr>, Box<TypeExpr>),
    Struct {
        name: String,
        fields: Vec<(String, TypeExpr)>,
        open: bool,
    },
    Variant {
        name: String,
        cases: Vec<(String, Option<TypeExpr>)>,
    },
    Ref(String),
}

#[inline(always)]
pub(super) fn has_type_children(ty: &TypeExpr) -> bool {
    !matches!(
        ty,
        TypeExpr::Datum
            | TypeExpr::Void
            | TypeExpr::I64
            | TypeExpr::F64
            | TypeExpr::Bool
            | TypeExpr::Char
            | TypeExpr::Text
            | TypeExpr::Ident
            | TypeExpr::Ref(_)
    )
}

pub(super) const SHALLOW_TYPE_DEPTH: usize = 32;

#[derive(Clone, Copy)]
pub(super) enum UnaryTypeKind {
    List,
    Array,
    Option,
}
