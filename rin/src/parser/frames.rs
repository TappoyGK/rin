//! 反復解析で保持する途中状態。
use super::*;

#[derive(Clone, Copy)]
pub(super) enum WrapKind {
    Some,
    Ok,
    Err,
}

pub(super) enum BodyTarget {
    Defn { name: String, params: Vec<String> },
    Fn { params: Vec<String> },
    Let { bindings: Vec<(String, Expr)> },
    When { cond: Box<Expr> },
}

pub(super) struct StructFieldFrame {
    pub(super) brace_start: Span,
    pub(super) span: Span,
    pub(super) name: String,
    pub(super) fields: Vec<(String, Expr)>,
    pub(super) key: String,
    pub(super) max_child: usize,
}

pub(super) struct BodyFrame {
    pub(super) start: Span,
    pub(super) body: Vec<Expr>,
    pub(super) max_child: usize,
    pub(super) target: BodyTarget,
}

pub(super) struct LetBindFrame {
    pub(super) start: Span,
    pub(super) bind_start: Span,
    pub(super) bindings: Vec<(String, Expr)>,
    pub(super) key: String,
    pub(super) max_child: usize,
}

pub(super) struct WithFieldFrame {
    pub(super) start: Span,
    pub(super) brace_start: Span,
    pub(super) target: Box<Expr>,
    pub(super) fields: Vec<(String, Expr)>,
    pub(super) key: String,
    pub(super) max_child: usize,
}

pub(super) struct MatchArmFrame {
    pub(super) start: Span,
    pub(super) scrutinee: Box<Expr>,
    pub(super) arms: Vec<MatchArm>,
    pub(super) arm_start: Span,
    pub(super) pattern: Pattern,
    pub(super) body: Vec<Expr>,
    pub(super) max_child: usize,
}

pub(super) enum ParseFrame {
    PostfixIndex {
        target: Box<Expr>,
        target_depth: usize,
    },
    List {
        start: Span,
        items: Vec<Expr>,
        max_child: usize,
    },
    ParenFirst {
        start: Span,
    },
    Tuple {
        start: Span,
        items: Vec<Expr>,
        max_child: usize,
    },
    Call {
        start: Span,
        callee: Box<Expr>,
        args: Vec<Expr>,
        max_child: usize,
    },
    BuiltinWrap {
        span: Span,
        kind: WrapKind,
    },
    StructField(Box<StructFieldFrame>),
    VariantData {
        span: Span,
        type_name: String,
        variant: String,
    },
    Def {
        start: Span,
        name: String,
    },
    Body(Box<BodyFrame>),
    LetBindVal(Box<LetBindFrame>),
    IfCond {
        start: Span,
    },
    IfThen {
        start: Span,
        cond: Box<Expr>,
        max_child: usize,
    },
    IfElse {
        start: Span,
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        max_child: usize,
    },
    WhenCond {
        start: Span,
    },
    WithTarget {
        start: Span,
    },
    WithFieldVal(Box<WithFieldFrame>),
    MatchScrutinee {
        start: Span,
    },
    MatchArmBody(Box<MatchArmFrame>),
}
