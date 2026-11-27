//! 作業領域 (`WorkArena`) 上の推論型表現・単一化・作業回数予算管理。
//!
//! すべての型変数・一時型ノード・単一化スタック・ローカルスキーマは `WorkArena` に確保し、
//! システムヒープ確保を行わず `memory_bytes` へ計上する。

use crate::bytecode::BuiltinId;
use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::mem::{AllocCore, PermArena, RawStr, RawVec, WorkArena};

use super::types::{
    PermTypeNode, SchemaDef, SchemaId, SchemaKind, TY_BOOL, TY_CHAR, TY_DATUM, TY_F64, TY_I64,
    TY_IDENT, TY_TEXT, TY_VOID, TypeEnv, TypeId, VariantCase,
};

/// 入力サイズ `N` に対する型検査作業回数予算の係数 `c`（予算 = `c * max(N, 1)`）。
pub(crate) const TYPECHECK_WORK_MULTIPLIER: usize = 64;

/// 作業領域上の推論型 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WorkTypeId(pub(crate) u32);

/// 作業領域上の型変数 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct VarId(pub(crate) u32);

/// 永続領域または現在評価単位内のスキーマ ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WorkSchemaId {
    Perm(SchemaId),
    Local(u32),
}

/// 作業領域上の型ノード。
#[derive(Debug, Clone, Copy)]
pub(crate) enum WorkTypeNode {
    Perm(TypeId),
    Var(VarId),
    List(WorkTypeId),
    Array(WorkTypeId),
    Option(WorkTypeId),
    Result {
        ok: WorkTypeId,
        err: WorkTypeId,
    },
    Tuple(RawVec<WorkTypeId>),
    Fn {
        params: RawVec<WorkTypeId>,
        ret: WorkTypeId,
    },
    Schema(WorkSchemaId),
    Builtin(BuiltinId),
    Module(u32),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum VarState {
    Unbound,
    Bound(WorkTypeId),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalVariantCase {
    pub(crate) name: RawStr,
    pub(crate) payload: Option<WorkTypeId>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum LocalSchemaKind {
    Struct {
        open: bool,
        type_name: RawStr,
        fields: RawVec<(RawStr, WorkTypeId)>,
    },
    Variant {
        type_name: RawStr,
        cases: RawVec<LocalVariantCase>,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalSchemaDef {
    pub(crate) name: RawStr,
    pub(crate) span: Span,
    pub(crate) kind: Option<LocalSchemaKind>,
    pub(crate) perm_id: Option<SchemaId>,
}

/// 組み込み 8 型の `WorkTypeId`（`WorkTypePool::new` 時に先頭 8 スロットへ配置）。
pub(crate) const WTY_VOID: WorkTypeId = WorkTypeId(0);
pub(crate) const WTY_I64: WorkTypeId = WorkTypeId(1);
pub(crate) const WTY_F64: WorkTypeId = WorkTypeId(2);
pub(crate) const WTY_BOOL: WorkTypeId = WorkTypeId(3);
pub(crate) const WTY_CHAR: WorkTypeId = WorkTypeId(4);
pub(crate) const WTY_TEXT: WorkTypeId = WorkTypeId(5);
pub(crate) const WTY_IDENT: WorkTypeId = WorkTypeId(6);
pub(crate) const WTY_DATUM: WorkTypeId = WorkTypeId(7);

pub(crate) struct WorkTypePool {
    pub(crate) nodes: RawVec<WorkTypeNode>,
    pub(crate) vars: RawVec<VarState>,
    pub(crate) local_schemas: RawVec<LocalSchemaDef>,
    pub(crate) budget_remaining: usize,
}

impl WorkTypePool {
    pub(crate) fn new(
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        budget: usize,
        span: Span,
    ) -> Result<Self, ErrRecorded> {
        let mut nodes = RawVec::new();
        let prims = [
            WorkTypeNode::Perm(TY_VOID),
            WorkTypeNode::Perm(TY_I64),
            WorkTypeNode::Perm(TY_F64),
            WorkTypeNode::Perm(TY_BOOL),
            WorkTypeNode::Perm(TY_CHAR),
            WorkTypeNode::Perm(TY_TEXT),
            WorkTypeNode::Perm(TY_IDENT),
            WorkTypeNode::Perm(TY_DATUM),
        ];
        for n in prims {
            if nodes.push_work(work, core, n).is_none() {
                return Err(oom(core, span));
            }
        }
        Ok(Self {
            nodes,
            vars: RawVec::new(),
            local_schemas: RawVec::new(),
            budget_remaining: budget,
        })
    }

    /// 型検査作業回数を 1 消費する。上限超過時は `ErrorKind::TooManySteps` (`[eval.limits]`)。
    #[inline]
    pub(crate) fn tick(&mut self, core: &mut AllocCore<'_>, span: Span) -> Result<(), ErrRecorded> {
        if self.budget_remaining == 0 {
            return Err(core.err_str(
                "eval.limits",
                ErrorKind::TooManySteps,
                span,
                "typecheck work limit exceeded",
            ));
        }
        self.budget_remaining -= 1;
        Ok(())
    }

    pub(crate) fn alloc_node(
        &mut self,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        node: WorkTypeNode,
        span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        if let WorkTypeNode::Perm(tid) = node {
            match tid {
                TY_VOID => return Ok(WTY_VOID),
                TY_I64 => return Ok(WTY_I64),
                TY_F64 => return Ok(WTY_F64),
                TY_BOOL => return Ok(WTY_BOOL),
                TY_CHAR => return Ok(WTY_CHAR),
                TY_TEXT => return Ok(WTY_TEXT),
                TY_IDENT => return Ok(WTY_IDENT),
                TY_DATUM => return Ok(WTY_DATUM),
                _ => {}
            }
        }
        let id = WorkTypeId(self.nodes.len() as u32);
        if self.nodes.push_work(work, core, node).is_none() {
            return Err(oom(core, span));
        }
        Ok(id)
    }

    pub(crate) fn fresh_var(
        &mut self,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        let vid = VarId(self.vars.len() as u32);
        if self.vars.push_work(work, core, VarState::Unbound).is_none() {
            return Err(oom(core, span));
        }
        self.alloc_node(work, core, WorkTypeNode::Var(vid), span)
    }

    #[inline]
    pub(crate) fn get(&self, id: WorkTypeId) -> WorkTypeNode {
        self.nodes.as_slice()[id.0 as usize]
    }

    /// 型変数リンクを辿って代表 `WorkTypeId` を返す（経路圧縮付き）。
    pub(crate) fn resolve(
        &mut self,
        mut id: WorkTypeId,
        core: &mut AllocCore<'_>,
        span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        loop {
            match self.get(id) {
                WorkTypeNode::Var(vid) => match self.vars.as_slice()[vid.0 as usize] {
                    VarState::Unbound => return Ok(id),
                    VarState::Bound(next) => {
                        self.tick(core, span)?;
                        if let WorkTypeNode::Var(next_v) = self.get(next)
                            && let VarState::Bound(next2) = self.vars.as_slice()[next_v.0 as usize]
                        {
                            self.vars.as_mut_slice()[vid.0 as usize] = VarState::Bound(next2);
                        }
                        id = next;
                    }
                },
                _ => return Ok(id),
            }
        }
    }

    /// `PermTypeNode` を 1 段階展開した正規化ビューを返す。
    pub(crate) fn unwrap_perm(
        &mut self,
        id: WorkTypeId,
        env: &TypeEnv,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        span: Span,
    ) -> Result<WorkTypeNode, ErrRecorded> {
        let id = self.resolve(id, core, span)?;
        match self.get(id) {
            WorkTypeNode::Perm(tid) => match env.get_type(tid) {
                PermTypeNode::Void
                | PermTypeNode::I64
                | PermTypeNode::F64
                | PermTypeNode::Bool
                | PermTypeNode::Char
                | PermTypeNode::Text
                | PermTypeNode::Ident
                | PermTypeNode::Datum => Ok(WorkTypeNode::Perm(tid)),
                PermTypeNode::List(inner) => {
                    let w = self.alloc_node(work, core, WorkTypeNode::Perm(inner), span)?;
                    Ok(WorkTypeNode::List(w))
                }
                PermTypeNode::Array(inner) => {
                    let w = self.alloc_node(work, core, WorkTypeNode::Perm(inner), span)?;
                    Ok(WorkTypeNode::Array(w))
                }
                PermTypeNode::Option(inner) => {
                    let w = self.alloc_node(work, core, WorkTypeNode::Perm(inner), span)?;
                    Ok(WorkTypeNode::Option(w))
                }
                PermTypeNode::Result { ok, err } => {
                    let wo = self.alloc_node(work, core, WorkTypeNode::Perm(ok), span)?;
                    let we = self.alloc_node(work, core, WorkTypeNode::Perm(err), span)?;
                    Ok(WorkTypeNode::Result { ok: wo, err: we })
                }
                PermTypeNode::Tuple(items) => {
                    let mut out = RawVec::new();
                    for &it in items.as_slice() {
                        let w = self.alloc_node(work, core, WorkTypeNode::Perm(it), span)?;
                        if out.push_work(work, core, w).is_none() {
                            return Err(oom(core, span));
                        }
                    }
                    Ok(WorkTypeNode::Tuple(out))
                }
                PermTypeNode::Fn { params, ret } => {
                    let mut out = RawVec::new();
                    for &p in params.as_slice() {
                        let w = self.alloc_node(work, core, WorkTypeNode::Perm(p), span)?;
                        if out.push_work(work, core, w).is_none() {
                            return Err(oom(core, span));
                        }
                    }
                    let wr = self.alloc_node(work, core, WorkTypeNode::Perm(ret), span)?;
                    Ok(WorkTypeNode::Fn {
                        params: out,
                        ret: wr,
                    })
                }
                PermTypeNode::Schema(sid) => Ok(WorkTypeNode::Schema(WorkSchemaId::Perm(sid))),
            },
            other => Ok(other),
        }
    }

    /// 6 基本型（`i64`, `f64`, `bool`, `char`, `text`, `ident`）かどうか。
    pub(crate) fn is_basic6(
        &mut self,
        id: WorkTypeId,
        core: &mut AllocCore<'_>,
        span: Span,
    ) -> Result<bool, ErrRecorded> {
        let id = self.resolve(id, core, span)?;
        Ok(matches!(
            self.get(id),
            WorkTypeNode::Perm(TY_I64 | TY_F64 | TY_BOOL | TY_CHAR | TY_TEXT | TY_IDENT)
        ))
    }

    /// 順序比較可能型（`i64`, `f64`, `text`, `ident`）かどうか。
    pub(crate) fn is_orderable(
        &mut self,
        id: WorkTypeId,
        core: &mut AllocCore<'_>,
        span: Span,
    ) -> Result<bool, ErrRecorded> {
        let id = self.resolve(id, core, span)?;
        Ok(matches!(
            self.get(id),
            WorkTypeNode::Perm(TY_I64 | TY_F64 | TY_TEXT | TY_IDENT)
        ))
    }

    /// 数値型（`i64`, `f64`）かどうか。
    pub(crate) fn is_numeric(
        &mut self,
        id: WorkTypeId,
        core: &mut AllocCore<'_>,
        span: Span,
    ) -> Result<bool, ErrRecorded> {
        let id = self.resolve(id, core, span)?;
        Ok(matches!(self.get(id), WorkTypeNode::Perm(TY_I64 | TY_F64)))
    }

    /// `datum` 期待位置への暗黙受け入れ判定（6 基本型または `datum`）。
    pub(crate) fn is_datum_compatible(
        &mut self,
        id: WorkTypeId,
        core: &mut AllocCore<'_>,
        span: Span,
    ) -> Result<bool, ErrRecorded> {
        let id = self.resolve(id, core, span)?;
        Ok(matches!(
            self.get(id),
            WorkTypeNode::Perm(TY_DATUM | TY_I64 | TY_F64 | TY_BOOL | TY_CHAR | TY_TEXT | TY_IDENT)
        ))
    }

    /// 出現検査（無限型の構築を防ぐ、非再帰スタック実装）。
    fn occurs(
        &mut self,
        target: VarId,
        root: WorkTypeId,
        env: &TypeEnv,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        span: Span,
    ) -> Result<bool, ErrRecorded> {
        let mut stack = RawVec::<WorkTypeId>::new();
        if stack.push_work(work, core, root).is_none() {
            return Err(oom(core, span));
        }
        while let Some(cur) = stack.pop() {
            self.tick(core, span)?;
            let cur = self.resolve(cur, core, span)?;
            match self.get(cur) {
                WorkTypeNode::Var(v) => {
                    if v == target {
                        return Ok(true);
                    }
                }
                WorkTypeNode::Perm(_)
                | WorkTypeNode::Schema(_)
                | WorkTypeNode::Builtin(_)
                | WorkTypeNode::Module(_) => {
                    // 永続型・スキーマ型は未束縛の作業型変数を含まない。
                    let _ = env;
                }
                WorkTypeNode::List(inner)
                | WorkTypeNode::Array(inner)
                | WorkTypeNode::Option(inner) => {
                    if stack.push_work(work, core, inner).is_none() {
                        return Err(oom(core, span));
                    }
                }
                WorkTypeNode::Result { ok, err } => {
                    if stack.push_work(work, core, ok).is_none()
                        || stack.push_work(work, core, err).is_none()
                    {
                        return Err(oom(core, span));
                    }
                }
                WorkTypeNode::Tuple(items) => {
                    for &it in items.as_slice() {
                        if stack.push_work(work, core, it).is_none() {
                            return Err(oom(core, span));
                        }
                    }
                }
                WorkTypeNode::Fn { params, ret } => {
                    for &p in params.as_slice() {
                        if stack.push_work(work, core, p).is_none() {
                            return Err(oom(core, span));
                        }
                    }
                    if stack.push_work(work, core, ret).is_none() {
                        return Err(oom(core, span));
                    }
                }
            }
        }
        Ok(false)
    }

    /// 2 つの型を単一化する（非再帰スタック実装）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn unify(
        &mut self,
        a: WorkTypeId,
        b: WorkTypeId,
        env: &TypeEnv,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        span: Span,
        rule: &'static str,
    ) -> Result<(), ErrRecorded> {
        let mut stack = RawVec::<(WorkTypeId, WorkTypeId)>::new();
        if stack.push_work(work, core, (a, b)).is_none() {
            return Err(oom(core, span));
        }
        while let Some((lhs, rhs)) = stack.pop() {
            self.tick(core, span)?;
            let lhs = self.resolve(lhs, core, span)?;
            let rhs = self.resolve(rhs, core, span)?;
            if lhs == rhs {
                continue;
            }
            let ln = self.unwrap_perm(lhs, env, work, core, span)?;
            let rn = self.unwrap_perm(rhs, env, work, core, span)?;
            match (ln, rn) {
                (WorkTypeNode::Var(va), WorkTypeNode::Var(vb)) => {
                    if va != vb {
                        self.vars.as_mut_slice()[va.0 as usize] = VarState::Bound(rhs);
                    }
                }
                (WorkTypeNode::Var(va), _) => {
                    if self.occurs(va, rhs, env, work, core, span)? {
                        return Err(core.err_str(
                            rule,
                            ErrorKind::Type,
                            span,
                            "recursive type cannot be unified",
                        ));
                    }
                    self.vars.as_mut_slice()[va.0 as usize] = VarState::Bound(rhs);
                }
                (_, WorkTypeNode::Var(vb)) => {
                    if self.occurs(vb, lhs, env, work, core, span)? {
                        return Err(core.err_str(
                            rule,
                            ErrorKind::Type,
                            span,
                            "recursive type cannot be unified",
                        ));
                    }
                    self.vars.as_mut_slice()[vb.0 as usize] = VarState::Bound(lhs);
                }
                (WorkTypeNode::Perm(pa), WorkTypeNode::Perm(pb)) if pa == pb => {}
                (WorkTypeNode::Schema(sa), WorkTypeNode::Schema(sb)) if sa == sb => {}
                (WorkTypeNode::List(la), WorkTypeNode::List(lb))
                | (WorkTypeNode::Array(la), WorkTypeNode::Array(lb))
                | (WorkTypeNode::Option(la), WorkTypeNode::Option(lb)) => {
                    if stack.push_work(work, core, (la, lb)).is_none() {
                        return Err(oom(core, span));
                    }
                }
                (
                    WorkTypeNode::Result { ok: oa, err: ea },
                    WorkTypeNode::Result { ok: ob, err: eb },
                ) => {
                    if stack.push_work(work, core, (oa, ob)).is_none()
                        || stack.push_work(work, core, (ea, eb)).is_none()
                    {
                        return Err(oom(core, span));
                    }
                }
                (WorkTypeNode::Tuple(ta), WorkTypeNode::Tuple(tb)) if ta.len() == tb.len() => {
                    for (&xa, &xb) in ta.as_slice().iter().zip(tb.as_slice()) {
                        if stack.push_work(work, core, (xa, xb)).is_none() {
                            return Err(oom(core, span));
                        }
                    }
                }
                (
                    WorkTypeNode::Fn {
                        params: pa,
                        ret: ra,
                    },
                    WorkTypeNode::Fn {
                        params: pb,
                        ret: rb,
                    },
                ) if pa.len() == pb.len() => {
                    for (&xa, &xb) in pa.as_slice().iter().zip(pb.as_slice()) {
                        if stack.push_work(work, core, (xa, xb)).is_none() {
                            return Err(oom(core, span));
                        }
                    }
                    if stack.push_work(work, core, (ra, rb)).is_none() {
                        return Err(oom(core, span));
                    }
                }
                _ => {
                    return Err(core.err_str(rule, ErrorKind::Type, span, "type mismatch"));
                }
            }
        }
        Ok(())
    }

    /// `expected` 位置に `actual` を適合させる（`datum` および `(list datum)` の暗黙受け入れを含む）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn coerce_to_expected(
        &mut self,
        actual: WorkTypeId,
        expected: WorkTypeId,
        env: &TypeEnv,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        span: Span,
        rule: &'static str,
    ) -> Result<WorkTypeId, ErrRecorded> {
        let exp_r = self.resolve(expected, core, span)?;
        let act_r = self.resolve(actual, core, span)?;

        // 1. 期待型が `datum`: 6 基本型・`datum` を暗黙に受け入れる
        if matches!(self.get(exp_r), WorkTypeNode::Perm(TY_DATUM))
            && self.is_datum_compatible(act_r, core, span)?
        {
            return Ok(WTY_DATUM);
        }

        // 2. 期待型が `(list datum)`: 6 基本型のリストも暗黙に受け入れる
        if let WorkTypeNode::List(exp_elem) = self.unwrap_perm(exp_r, env, work, core, span)? {
            let exp_elem_r = self.resolve(exp_elem, core, span)?;
            if matches!(self.get(exp_elem_r), WorkTypeNode::Perm(TY_DATUM))
                && let WorkTypeNode::List(act_elem) =
                    self.unwrap_perm(act_r, env, work, core, span)?
                && self.is_datum_compatible(act_elem, core, span)?
            {
                return Ok(exp_r);
            }
        }

        // 3. 通常の単一化
        self.unify(act_r, exp_r, env, work, core, span, rule)?;
        Ok(exp_r)
    }

    /// 未束縛の型変数が含まれているか検査する（トップレベル `def` の推論確定チェック用）。
    pub(crate) fn has_unbound_var(
        &mut self,
        root: WorkTypeId,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        span: Span,
    ) -> Result<bool, ErrRecorded> {
        let mut stack = RawVec::<WorkTypeId>::new();
        if stack.push_work(work, core, root).is_none() {
            return Err(oom(core, span));
        }
        while let Some(cur) = stack.pop() {
            self.tick(core, span)?;
            let cur = self.resolve(cur, core, span)?;
            match self.get(cur) {
                WorkTypeNode::Var(_) => return Ok(true),
                WorkTypeNode::Perm(_)
                | WorkTypeNode::Schema(_)
                | WorkTypeNode::Builtin(_)
                | WorkTypeNode::Module(_) => {}
                WorkTypeNode::List(inner)
                | WorkTypeNode::Array(inner)
                | WorkTypeNode::Option(inner) => {
                    if stack.push_work(work, core, inner).is_none() {
                        return Err(oom(core, span));
                    }
                }
                WorkTypeNode::Result { ok, err } => {
                    if stack.push_work(work, core, ok).is_none()
                        || stack.push_work(work, core, err).is_none()
                    {
                        return Err(oom(core, span));
                    }
                }
                WorkTypeNode::Tuple(items) => {
                    for &it in items.as_slice() {
                        if stack.push_work(work, core, it).is_none() {
                            return Err(oom(core, span));
                        }
                    }
                }
                WorkTypeNode::Fn { params, ret } => {
                    for &p in params.as_slice() {
                        if stack.push_work(work, core, p).is_none() {
                            return Err(oom(core, span));
                        }
                    }
                    if stack.push_work(work, core, ret).is_none() {
                        return Err(oom(core, span));
                    }
                }
            }
        }
        Ok(false)
    }

    /// 確定済みの `WorkTypeId` を `PermArena` 上の `TypeId` へ変換して登録する。
    pub(crate) fn commit_type(
        &mut self,
        id: WorkTypeId,
        env: &mut TypeEnv,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        span: Span,
    ) -> Result<TypeId, ErrRecorded> {
        self.tick(core, span)?;
        let id = self.resolve(id, core, span)?;
        match self.get(id) {
            WorkTypeNode::Perm(tid) => Ok(tid),
            WorkTypeNode::Schema(WorkSchemaId::Perm(sid)) => {
                env.alloc_type(perm, core, PermTypeNode::Schema(sid), span)
            }
            WorkTypeNode::Schema(WorkSchemaId::Local(lidx)) => {
                let sid = self.local_schemas.as_slice()[lidx as usize]
                    .perm_id
                    .expect("local schema must be committed before types");
                env.alloc_type(perm, core, PermTypeNode::Schema(sid), span)
            }
            WorkTypeNode::List(inner) => {
                let t = self.commit_type(inner, env, perm, core, span)?;
                env.alloc_type(perm, core, PermTypeNode::List(t), span)
            }
            WorkTypeNode::Array(inner) => {
                let t = self.commit_type(inner, env, perm, core, span)?;
                env.alloc_type(perm, core, PermTypeNode::Array(t), span)
            }
            WorkTypeNode::Option(inner) => {
                let t = self.commit_type(inner, env, perm, core, span)?;
                env.alloc_type(perm, core, PermTypeNode::Option(t), span)
            }
            WorkTypeNode::Result { ok, err } => {
                let to = self.commit_type(ok, env, perm, core, span)?;
                let te = self.commit_type(err, env, perm, core, span)?;
                env.alloc_type(perm, core, PermTypeNode::Result { ok: to, err: te }, span)
            }
            WorkTypeNode::Tuple(items) => {
                let mut out = RawVec::new();
                for &it in items.as_slice() {
                    let t = self.commit_type(it, env, perm, core, span)?;
                    if out.push_perm(perm, core, t).is_none() {
                        return Err(oom(core, span));
                    }
                }
                env.alloc_type(perm, core, PermTypeNode::Tuple(out), span)
            }
            WorkTypeNode::Fn { params, ret } => {
                let mut out = RawVec::new();
                for &p in params.as_slice() {
                    let t = self.commit_type(p, env, perm, core, span)?;
                    if out.push_perm(perm, core, t).is_none() {
                        return Err(oom(core, span));
                    }
                }
                let tr = self.commit_type(ret, env, perm, core, span)?;
                env.alloc_type(
                    perm,
                    core,
                    PermTypeNode::Fn {
                        params: out,
                        ret: tr,
                    },
                    span,
                )
            }
            WorkTypeNode::Var(_) | WorkTypeNode::Builtin(_) | WorkTypeNode::Module(_) => Err(core
                .err_str(
                    "type.builtin",
                    ErrorKind::Type,
                    span,
                    "cannot determine concrete type",
                )),
        }
    }

    /// 現在の評価単位で定義された全 `LocalSchemaDef` を `PermArena` へ 2 パスで確定する
    /// （相互再帰・前方参照に対応するため、まず全 `SchemaId` を確保してからフィールド型を確定する）。
    pub(crate) fn commit_local_schemas(
        &mut self,
        module_name: Option<RawStr>,
        env: &mut TypeEnv,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
    ) -> Result<(), ErrRecorded> {
        let count = self.local_schemas.len();
        // パス 1: プレースホルダを確保して `perm_id` を確定
        for i in 0..count {
            let ls = self.local_schemas.as_slice()[i];
            let Some(name_perm) = RawStr::from_perm(perm, core, ls.name.as_str()) else {
                return Err(oom(core, ls.span));
            };
            let placeholder = SchemaDef {
                name: name_perm,
                module: module_name,
                kind: SchemaKind::Struct {
                    open: false,
                    type_name: name_perm,
                    fields: RawVec::new(),
                },
            };
            let sid = env.alloc_schema(perm, core, placeholder, ls.span)?;
            self.local_schemas.as_mut_slice()[i].perm_id = Some(sid);
        }
        // パス 2: フィールド型・バリアントペイロード型を確定して書き戻す
        for i in 0..count {
            let ls = self.local_schemas.as_slice()[i];
            let sid = ls.perm_id.expect("perm_id assigned in pass 1");
            let kind = ls.kind.expect("local schema kind resolved");
            let perm_kind = match kind {
                LocalSchemaKind::Struct {
                    open,
                    type_name,
                    fields,
                } => {
                    let Some(tn_perm) = RawStr::from_perm(perm, core, type_name.as_str()) else {
                        return Err(oom(core, ls.span));
                    };
                    let mut perm_fields = RawVec::new();
                    for &(fname, fty) in fields.as_slice() {
                        let Some(fn_perm) = RawStr::from_perm(perm, core, fname.as_str()) else {
                            return Err(oom(core, ls.span));
                        };
                        let pt = self.commit_type(fty, env, perm, core, ls.span)?;
                        if perm_fields.push_perm(perm, core, (fn_perm, pt)).is_none() {
                            return Err(oom(core, ls.span));
                        }
                    }
                    SchemaKind::Struct {
                        open,
                        type_name: tn_perm,
                        fields: perm_fields,
                    }
                }
                LocalSchemaKind::Variant { type_name, cases } => {
                    let Some(tn_perm) = RawStr::from_perm(perm, core, type_name.as_str()) else {
                        return Err(oom(core, ls.span));
                    };
                    let mut perm_cases = RawVec::new();
                    for &c in cases.as_slice() {
                        let Some(cn_perm) = RawStr::from_perm(perm, core, c.name.as_str()) else {
                            return Err(oom(core, ls.span));
                        };
                        let payload = match c.payload {
                            Some(pty) => Some(self.commit_type(pty, env, perm, core, ls.span)?),
                            None => None,
                        };
                        if perm_cases
                            .push_perm(
                                perm,
                                core,
                                VariantCase {
                                    name: cn_perm,
                                    payload,
                                },
                            )
                            .is_none()
                        {
                            return Err(oom(core, ls.span));
                        }
                    }
                    SchemaKind::Variant {
                        type_name: tn_perm,
                        cases: perm_cases,
                    }
                }
            };
            env.get_schema_mut(sid).kind = perm_kind;
        }
        Ok(())
    }
}

#[inline]
pub(crate) fn oom(core: &mut AllocCore<'_>, span: Span) -> ErrRecorded {
    core.err_str(
        "eval.limits",
        ErrorKind::TooMuchMemory,
        span,
        "memory limit exceeded",
    )
}
