//! 反復解析で保持する途中状態（作業領域 `WorkArena` 上で管理）。
use super::*;

#[derive(Clone, Copy)]
pub(super) enum WrapKind {
    Some,
    Ok,
    Err,
}

#[derive(Clone, Copy)]
pub(super) enum BodyTarget {
    Defn {
        name: RawStr,
        params: RawVec<RawStr>,
    },
    Fn {
        params: RawVec<RawStr>,
    },
    Let {
        bindings: RawVec<(RawStr, Expr)>,
    },
    When {
        cond: WorkBox<Expr>,
    },
}

#[derive(Clone, Copy)]
pub(super) struct StructFieldFrame {
    pub(super) brace_start: Span,
    pub(super) span: Span,
    pub(super) name: RawStr,
    pub(super) fields: RawVec<(RawStr, Expr)>,
    pub(super) key: RawStr,
    pub(super) max_child: usize,
    pub(super) sexp: bool,
}

#[derive(Clone, Copy)]
pub(super) struct BodyFrame {
    pub(super) start: Span,
    pub(super) body: RawVec<Expr>,
    pub(super) max_child: usize,
    pub(super) target: BodyTarget,
}

#[derive(Clone, Copy)]
pub(super) struct LetBindFrame {
    pub(super) start: Span,
    pub(super) bind_start: Span,
    pub(super) bindings: RawVec<(RawStr, Expr)>,
    pub(super) key: RawStr,
    pub(super) max_child: usize,
}

#[derive(Clone, Copy)]
pub(super) struct WithFieldFrame {
    pub(super) start: Span,
    pub(super) brace_start: Span,
    pub(super) target: WorkBox<Expr>,
    pub(super) fields: RawVec<(RawStr, Expr)>,
    pub(super) key: RawStr,
    pub(super) max_child: usize,
    pub(super) sexp: bool,
}

#[derive(Clone, Copy)]
pub(super) struct MatchArmFrame {
    pub(super) start: Span,
    pub(super) scrutinee: WorkBox<Expr>,
    pub(super) arms: RawVec<MatchArm>,
    pub(super) arm_start: Span,
    pub(super) pattern: Pattern,
    pub(super) body: RawVec<Expr>,
    pub(super) max_child: usize,
}

#[derive(Clone, Copy)]
pub(super) struct QqState {
    pub(super) depth: u32,
    pub(super) eval: bool,
}

impl QqState {
    pub(super) const QUOTE: Self = Self {
        depth: 0,
        eval: false,
    };
    pub(super) const QUASIQUOTE: Self = Self {
        depth: 1,
        eval: true,
    };

    #[inline]
    pub(super) const fn enter_qq(self) -> Self {
        Self {
            depth: self.depth + 1,
            eval: self.eval,
        }
    }

    #[inline]
    pub(super) const fn enter_unquote(self) -> Self {
        Self {
            depth: self.depth - 1,
            eval: self.eval,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct DatumListFrame {
    pub(super) start: Span,
    pub(super) items: RawVec<Datum>,
    pub(super) max_child: usize,
    pub(super) qq: QqState,
    pub(super) close: WorkTokenKind,
}

#[derive(Clone, Copy)]
pub(super) struct DatumStructFrame {
    pub(super) start: Span,
    pub(super) brace_start: Span,
    pub(super) items: RawVec<Datum>,
    pub(super) key: RawStr,
    pub(super) key_span: Span,
    pub(super) max_child: usize,
    pub(super) qq: QqState,
    pub(super) is_with: bool,
}

#[derive(Clone, Copy)]
pub(super) enum ParseFrame {
    PostfixIndex {
        target: WorkBox<Expr>,
        target_depth: usize,
    },
    FieldTarget {
        start: Span,
    },
    IndexTarget {
        start: Span,
    },
    IndexArg {
        start: Span,
        target: WorkBox<Expr>,
        target_depth: usize,
    },
    List {
        start: Span,
        items: RawVec<Expr>,
        max_child: usize,
    },
    ParenFirst {
        start: Span,
    },
    Tuple {
        start: Span,
        items: RawVec<Expr>,
        max_child: usize,
    },
    Call {
        start: Span,
        callee: WorkBox<Expr>,
        args: RawVec<Expr>,
        max_child: usize,
    },
    BuiltinWrap {
        span: Span,
        kind: WrapKind,
    },
    StructField(WorkBox<StructFieldFrame>),
    VariantData {
        span: Span,
        type_name: RawStr,
        variant: RawStr,
    },
    Def {
        start: Span,
        name: RawStr,
    },
    Body(WorkBox<BodyFrame>),
    LetBindVal(WorkBox<LetBindFrame>),
    IfCond {
        start: Span,
    },
    IfThen {
        start: Span,
        cond: WorkBox<Expr>,
        max_child: usize,
    },
    IfElse {
        start: Span,
        cond: WorkBox<Expr>,
        then_branch: WorkBox<Expr>,
        max_child: usize,
    },
    WhenCond {
        start: Span,
    },
    WithTarget {
        start: Span,
    },
    WithFieldVal(WorkBox<WithFieldFrame>),
    MatchScrutinee {
        start: Span,
    },
    MatchArmBody(WorkBox<MatchArmFrame>),
    QuoteFinish {
        span: Span,
        is_paren: bool,
        is_quasiquote: bool,
    },
    DatumUnquote {
        span: Span,
        is_paren: bool,
        is_splicing: bool,
    },
    DatumWrap {
        span: Span,
        tag: &'static str,
        is_paren: bool,
        outer_qq: QqState,
    },
    DatumList(WorkBox<DatumListFrame>),
    DatumIndex {
        target: WorkBox<Datum>,
        target_depth: usize,
        qq: QqState,
    },
    DatumVariantData {
        span: Span,
        type_name: RawStr,
        variant: RawStr,
    },
    DatumStructFields(WorkBox<DatumStructFrame>),
}
