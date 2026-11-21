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
