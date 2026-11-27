//! ゼロヒープ双方向型検査器本体。
//!
//! すべての作業データ（スコープ・型変数・単一化・網羅性フラグ）は `WorkArena` 上で管理し、
//! 型検査全体が成功した後にのみ確定済みの型定義・シグネチャを `PermArena` へ書き込む。

use crate::ast::{
    Datum, DatumKind, DatumTypePat, DefSchemaExpr, Expr, ExprKind, MatchArm, Pattern, SchemaBody,
    TypeExpr, TypeExprKind,
};
use crate::bytecode::BuiltinId;
use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::mem::{AllocCore, PermArena, RawMap, RawStr, RawVec, WorkArena, fnv1a_bytes};

use super::types::{
    FnSig, ModuleTypeEnv, SchemaKind, TY_DATUM, TY_TEXT, TypeEnv, TypeId, ValBinding,
    is_builtin_type_name,
};
use super::work_types::{
    LocalSchemaDef, LocalSchemaKind, LocalVariantCase, TYPECHECK_WORK_MULTIPLIER, WTY_BOOL,
    WTY_CHAR, WTY_DATUM, WTY_F64, WTY_I64, WTY_IDENT, WTY_TEXT, WTY_VOID, WorkSchemaId, WorkTypeId,
    WorkTypeNode, WorkTypePool, oom,
};

/// 型検査の実行オプション。
#[derive(Clone, Copy)]
pub(crate) struct CheckOptions<'a> {
    /// 凍結環境（`Machine`）上での評価かどうか。
    pub(crate) frozen: bool,
    /// `defn` の引数型・戻り値型注釈を必須とするか（STEP 3 で `true`）。
    pub(crate) strict_defn_annotations: bool,
    /// モジュール読み込み時はモジュール名、ルート評価時は `None`。
    pub(crate) module_name: Option<&'a str>,
    /// テストランナー（`deftest` / `check` / `check_eq`）を有効にするか。
    pub(crate) test_mode: bool,
    /// 成功時に新規定義を `TypeEnv`（`PermArena`）へ確定するか。
    pub(crate) commit: bool,
    /// 一時的な初期束縛（`eval_with` 用）。
    pub(crate) extra_bindings: &'a [(&'a str, TypeId)],
}

impl Default for CheckOptions<'_> {
    fn default() -> Self {
        Self {
            frozen: false,
            strict_defn_annotations: true,
            module_name: None,
            test_mode: false,
            commit: true,
            extra_bindings: &[],
        }
    }
}

#[derive(Clone, Copy)]
struct LocalDefnInfo {
    name: RawStr,
    span: Span,
    params: RawVec<(RawStr, WorkTypeId)>,
    ret: WorkTypeId,
    fn_ty: WorkTypeId,
}

#[derive(Clone, Copy)]
struct LocalDefInfo {
    name: RawStr,
    span: Span,
    ty: WorkTypeId,
}

#[derive(Clone, Copy)]
struct ScopeBinding {
    name: RawStr,
    ty: WorkTypeId,
}

pub(crate) struct TypeChecker<'a, 'alloc> {
    pub(super) env: &'a mut TypeEnv,
    pub(super) perm: &'a mut PermArena,
    pub(super) work: &'a mut WorkArena,
    pub(super) core: &'a mut AllocCore<'alloc>,
    pub(super) wpool: WorkTypePool,
    opts: CheckOptions<'a>,
    /// ローカル `defschema` 名 -> `local_schemas` インデックス。
    local_type_map: RawMap<RawStr, u32>,
    /// ローカル `def` / `defn` 名 -> 推論型。
    local_val_map: RawMap<RawStr, WorkTypeId>,
    /// 定義順の `def` 一覧。
    local_defs: RawVec<LocalDefInfo>,
    /// 定義順の `defn` 一覧。
    local_defns: RawVec<LocalDefnInfo>,
    /// `import` されたモジュール名 -> `TypeEnv.module_slots` インデックス。
    imported_modules: RawMap<RawStr, u32>,
    /// ブロックスコープの変数スタック（`let` / `fn` / `defn` / `match`）。
    scopes: RawVec<ScopeBinding>,
    /// `export` で指定された名前一覧。
    explicit_exports: RawVec<(RawStr, Span)>,
    has_export: bool,
}

/// プログラム AST 列を型検査する。
pub(crate) fn check_program_work<'a, 'alloc>(
    exprs: &[Expr],
    env: &'a mut TypeEnv,
    perm: &'a mut PermArena,
    work: &'a mut WorkArena,
    core: &'a mut AllocCore<'alloc>,
    opts: CheckOptions<'a>,
) -> Result<WorkTypeId, ErrRecorded> {
    let first_span = exprs.first().map_or(Span::default(), |e| e.span);
    let node_count = count_program_nodes(exprs, work, core, first_span)?;
    let budget = node_count.max(1).saturating_mul(TYPECHECK_WORK_MULTIPLIER);
    let wpool = WorkTypePool::new(work, core, budget, first_span)?;

    let mut tc = TypeChecker {
        env,
        perm,
        work,
        core,
        wpool,
        opts,
        local_type_map: RawMap::new(),
        local_val_map: RawMap::new(),
        local_defs: RawVec::new(),
        local_defns: RawVec::new(),
        imported_modules: RawMap::new(),
        scopes: RawVec::new(),
        explicit_exports: RawVec::new(),
        has_export: false,
    };

    tc.run(exprs)
}

/// 単独の `TypeExpr` を `TypeEnv` 上で解決して `TypeId` を返す（ホスト境界 API / `resolve_type` 用）。
pub(crate) fn resolve_type_expr_perm<'a, 'alloc>(
    ty_expr: &TypeExpr,
    env: &'a mut TypeEnv,
    perm: &'a mut PermArena,
    work: &'a mut WorkArena,
    core: &'a mut AllocCore<'alloc>,
) -> Result<TypeId, ErrRecorded> {
    let nodes = count_type_expr_nodes(ty_expr, work, core)?.max(1);
    let wpool = WorkTypePool::new(
        work,
        core,
        nodes.saturating_mul(TYPECHECK_WORK_MULTIPLIER),
        ty_expr.span,
    )?;
    let mut tc = TypeChecker {
        env,
        perm,
        work,
        core,
        wpool,
        opts: CheckOptions {
            frozen: false,
            strict_defn_annotations: true,
            module_name: None,
            test_mode: false,
            commit: false,
            extra_bindings: &[],
        },
        local_type_map: RawMap::new(),
        local_val_map: RawMap::new(),
        local_defs: RawVec::new(),
        local_defns: RawVec::new(),
        imported_modules: RawMap::new(),
        scopes: RawVec::new(),
        explicit_exports: RawVec::new(),
        has_export: false,
    };
    let wty = tc.resolve_type_expr(ty_expr)?;
    tc.wpool
        .commit_type(wty, tc.env, tc.perm, tc.core, ty_expr.span)
}

impl<'a> TypeChecker<'a, '_> {
    fn run(&mut self, exprs: &[Expr]) -> Result<WorkTypeId, ErrRecorded> {
        // 0. extra_bindings（eval_with の一時束縛）を最外スコープに積む
        for &(name, tid) in self.opts.extra_bindings {
            let Some(rs) = RawStr::from_work(self.work, self.core, name) else {
                return Err(oom(self.core, Span::default()));
            };
            let wty = self.wpool.alloc_node(
                self.work,
                self.core,
                WorkTypeNode::Perm(tid),
                Span::default(),
            )?;
            if self
                .scopes
                .push_work(self.work, self.core, ScopeBinding { name: rs, ty: wty })
                .is_none()
            {
                return Err(oom(self.core, Span::default()));
            }
        }

        // 1. import の収集（モジュール型参照を可能にするため defschema 解決より先に登録）
        for expr in exprs {
            if let ExprKind::Import { name } = expr.kind {
                let Some((slot_idx, _)) = self.env.lookup_module(name.as_str()) else {
                    return Err(self.core.err_str(
                        "expr.module",
                        ErrorKind::Runtime,
                        expr.span,
                        "unknown module",
                    ));
                };
                let h = fnv1a_bytes(name.as_bytes());
                if self
                    .imported_modules
                    .insert_work(self.work, self.core, h, name, slot_idx, |a, b| {
                        a.as_str() == b.as_str()
                    })
                    .is_none()
                {
                    return Err(oom(self.core, expr.span));
                }
            }
        }

        // 2. defschema 名の事前登録（前方参照・相互再帰を許可）
        for expr in exprs {
            if let ExprKind::DefSchema(ds) = expr.kind {
                if self.opts.frozen {
                    return Err(self.core.err_str(
                        "env.frozen",
                        ErrorKind::Runtime,
                        expr.span,
                        "cannot use defschema in frozen environment",
                    ));
                }
                self.preregister_defschema(ds.as_ref(), expr.span)?;
            }
        }

        // 3. defschema 本体の解決（CloneOf の循環検査を含む）
        let mut schema_ast_list = RawVec::<(&DefSchemaExpr, Span)>::new();
        for expr in exprs {
            if let ExprKind::DefSchema(ds) = &expr.kind
                && schema_ast_list
                    .push_work(self.work, self.core, (ds.as_ref(), expr.span))
                    .is_none()
            {
                return Err(oom(self.core, expr.span));
            }
        }
        self.resolve_all_defschemas(schema_ast_list.as_slice())?;

        // 4. defn シグネチャの事前登録（相互再帰・前方呼び出しを許可）
        for expr in exprs {
            if let ExprKind::Defn(d) = expr.kind {
                if self.opts.frozen {
                    return Err(self.core.err_str(
                        "env.frozen",
                        ErrorKind::Runtime,
                        expr.span,
                        "cannot use defn in frozen environment",
                    ));
                }
                self.preregister_defn(d.as_ref(), expr.span)?;
            }
        }

        // 5. トップレベル式を順に型検査
        let mut last_ty = WTY_VOID;
        let mut defn_cursor = 0usize;
        for expr in exprs {
            self.wpool.tick(self.core, expr.span)?;
            last_ty = match &expr.kind {
                ExprKind::DefSchema(_) | ExprKind::Import { .. } => WTY_VOID,
                ExprKind::Export { names } => {
                    self.has_export = true;
                    for &n in names.as_slice() {
                        if self
                            .explicit_exports
                            .push_work(self.work, self.core, (n, expr.span))
                            .is_none()
                        {
                            return Err(oom(self.core, expr.span));
                        }
                    }
                    WTY_VOID
                }
                ExprKind::Def(d) => {
                    if self.opts.frozen {
                        return Err(self.core.err_str(
                            "env.frozen",
                            ErrorKind::Runtime,
                            expr.span,
                            "cannot use def in frozen environment",
                        ));
                    }
                    let d = d.as_ref();
                    self.check_no_val_redef(d.name.as_str(), expr.span)?;
                    let exp = match d.ty {
                        Some(ty_box) => Some(self.resolve_type_expr(ty_box.as_ref())?),
                        None => None,
                    };
                    let val_ty = self.check_expr(d.value.as_ref(), exp)?;
                    let final_ty = match exp {
                        Some(et) => self.wpool.coerce_to_expected(
                            val_ty, et, self.env, self.work, self.core, expr.span, "expr.def",
                        )?,
                        None => val_ty,
                    };
                    if self
                        .wpool
                        .has_unbound_var(final_ty, self.work, self.core, expr.span)?
                    {
                        return Err(self.type_err(
                            "expr.def",
                            expr.span,
                            "cannot infer concrete type for def; add a type annotation",
                        ));
                    }
                    let h = fnv1a_bytes(d.name.as_bytes());
                    if self
                        .local_val_map
                        .insert_work(self.work, self.core, h, d.name, final_ty, |a, b| {
                            a.as_str() == b.as_str()
                        })
                        .is_none()
                    {
                        return Err(oom(self.core, expr.span));
                    }
                    if self
                        .local_defs
                        .push_work(
                            self.work,
                            self.core,
                            LocalDefInfo {
                                name: d.name,
                                span: expr.span,
                                ty: final_ty,
                            },
                        )
                        .is_none()
                    {
                        return Err(oom(self.core, expr.span));
                    }
                    WTY_VOID
                }
                ExprKind::Defn(d) => {
                    let info = self.local_defns.as_slice()[defn_cursor];
                    defn_cursor += 1;
                    self.check_defn_body(d.as_ref(), info, expr.span)?;
                    WTY_VOID
                }
                _ => self.check_expr(expr, None)?,
            };
        }

        // 6. export 名の存在確認
        for i in 0..self.explicit_exports.len() {
            let (name, span) = self.explicit_exports.as_slice()[i];
            let h = fnv1a_bytes(name.as_bytes());
            let in_val = self
                .local_val_map
                .get_by(h, |k| k.as_str() == name.as_str())
                .is_some();
            let in_ty = self
                .local_type_map
                .get_by(h, |k| k.as_str() == name.as_str())
                .is_some();
            if !in_val && !in_ty {
                return Err(self.type_err("expr.module", span, "exported name is not defined"));
            }
        }

        // 7. 永続領域 (`PermArena`) への確定書き込み（全検査成功後）
        if self.opts.commit && !self.opts.frozen {
            self.commit_to_perm()?;
        }

        Ok(last_ty)
    }

    fn preregister_defschema(&mut self, ds: &DefSchemaExpr, span: Span) -> Result<(), ErrRecorded> {
        let name = ds.name.as_str();
        if is_builtin_type_name(name) {
            return Err(self.type_err("type.defschema", span, "cannot redefine builtin type name"));
        }
        let h = fnv1a_bytes(name.as_bytes());
        if self
            .local_type_map
            .get_by(h, |k| k.as_str() == name)
            .is_some()
        {
            return Err(self.type_err(
                "type.defschema",
                span,
                "duplicate defschema name in same unit",
            ));
        }
        if self.opts.module_name.is_none() && self.env.lookup_root_type(name).is_some() {
            return Err(self.type_err(
                "type.defschema",
                span,
                "cannot redefine existing schema type",
            ));
        }
        let idx = self.wpool.local_schemas.len() as u32;
        if self
            .wpool
            .local_schemas
            .push_work(
                self.work,
                self.core,
                LocalSchemaDef {
                    name: ds.name,
                    span,
                    kind: None,
                    perm_id: None,
                },
            )
            .is_none()
        {
            return Err(oom(self.core, span));
        }
        if self
            .local_type_map
            .insert_work(self.work, self.core, h, ds.name, idx, |a, b| {
                a.as_str() == b.as_str()
            })
            .is_none()
        {
            return Err(oom(self.core, span));
        }
        Ok(())
    }

    fn resolve_all_defschemas(
        &mut self,
        list: &[(&DefSchemaExpr, Span)],
    ) -> Result<(), ErrRecorded> {
        // 1. Struct / Variant の本体を先に解決する
        for (i, &(ds, span)) in list.iter().enumerate() {
            match &ds.body {
                SchemaBody::Struct {
                    open,
                    type_name,
                    fields,
                } => {
                    let mut wfields = RawVec::<(RawStr, WorkTypeId)>::new();
                    for &(fname, ref fty_expr) in fields.as_slice() {
                        // 重複フィールド検査
                        for &(prev, _) in wfields.as_slice() {
                            if prev.as_str() == fname.as_str() {
                                return Err(self.type_err(
                                    "type.defschema",
                                    fty_expr.span,
                                    "duplicate field name in struct schema",
                                ));
                            }
                        }
                        let fty = self.resolve_type_expr(fty_expr)?;
                        if wfields
                            .push_work(self.work, self.core, (fname, fty))
                            .is_none()
                        {
                            return Err(oom(self.core, span));
                        }
                    }
                    self.wpool.local_schemas.as_mut_slice()[i].kind =
                        Some(LocalSchemaKind::Struct {
                            open: *open,
                            type_name: *type_name,
                            fields: wfields,
                        });
                }
                SchemaBody::Variant { type_name, cases } => {
                    let mut wcases = RawVec::new();
                    for c in cases.as_slice() {
                        for prev in wcases.as_slice() {
                            let prev: &LocalVariantCase = prev;
                            if prev.name.as_str() == c.name.as_str() {
                                return Err(self.type_err(
                                    "type.defschema",
                                    c.span,
                                    "duplicate variant case name",
                                ));
                            }
                        }
                        let payload = match c.payload {
                            Some(pb) => Some(self.resolve_type_expr(pb.as_ref())?),
                            None => None,
                        };
                        if wcases
                            .push_work(
                                self.work,
                                self.core,
                                LocalVariantCase {
                                    name: c.name,
                                    payload,
                                },
                            )
                            .is_none()
                        {
                            return Err(oom(self.core, c.span));
                        }
                    }
                    self.wpool.local_schemas.as_mut_slice()[i].kind =
                        Some(LocalSchemaKind::Variant {
                            type_name: *type_name,
                            cases: wcases,
                        });
                }
                SchemaBody::CloneOf { .. } => {}
            }
        }

        // 2. CloneOf を反復的に解決（元の定義へ到達できない循環参照を検出して拒否）
        let n = list.len();
        for _round in 0..=n {
            let mut unresolved = 0usize;
            let mut progressed = false;
            for (i, &(ds, span)) in list.iter().enumerate() {
                if self.wpool.local_schemas.as_slice()[i].kind.is_some() {
                    continue;
                }
                let SchemaBody::CloneOf { module, name } = ds.body else {
                    continue;
                };
                match self.lookup_schema_ref(module, name, span)? {
                    WorkSchemaId::Perm(pid) => {
                        let cloned = self.clone_perm_schema_to_local(pid, ds.name, span)?;
                        self.wpool.local_schemas.as_mut_slice()[i].kind = Some(cloned);
                        progressed = true;
                    }
                    WorkSchemaId::Local(target_idx) => {
                        if let Some(target_kind) =
                            self.wpool.local_schemas.as_slice()[target_idx as usize].kind
                        {
                            let cloned = match target_kind {
                                LocalSchemaKind::Struct { open, fields, .. } => {
                                    LocalSchemaKind::Struct {
                                        open,
                                        type_name: ds.name,
                                        fields,
                                    }
                                }
                                LocalSchemaKind::Variant { cases, .. } => {
                                    LocalSchemaKind::Variant {
                                        type_name: ds.name,
                                        cases,
                                    }
                                }
                            };
                            self.wpool.local_schemas.as_mut_slice()[i].kind = Some(cloned);
                            progressed = true;
                        } else {
                            unresolved += 1;
                        }
                    }
                }
            }
            if unresolved == 0 {
                return Ok(());
            }
            if !progressed {
                // 循環参照
                for (i, &(_, span)) in list.iter().enumerate() {
                    if self.wpool.local_schemas.as_slice()[i].kind.is_none() {
                        return Err(self.type_err(
                            "type.defschema",
                            span,
                            "cyclic defschema clone reference",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn clone_perm_schema_to_local(
        &mut self,
        pid: super::types::SchemaId,
        new_name: RawStr,
        span: Span,
    ) -> Result<LocalSchemaKind, ErrRecorded> {
        let def = *self.env.get_schema(pid);
        match def.kind {
            SchemaKind::Struct { open, fields, .. } => {
                let mut wfields = RawVec::new();
                for &(fname, fty) in fields.as_slice() {
                    let wt = self.wpool.alloc_node(
                        self.work,
                        self.core,
                        WorkTypeNode::Perm(fty),
                        span,
                    )?;
                    if wfields
                        .push_work(self.work, self.core, (fname, wt))
                        .is_none()
                    {
                        return Err(oom(self.core, span));
                    }
                }
                Ok(LocalSchemaKind::Struct {
                    open,
                    type_name: new_name,
                    fields: wfields,
                })
            }
            SchemaKind::Variant { cases, .. } => {
                let mut wcases = RawVec::new();
                for &c in cases.as_slice() {
                    let payload = match c.payload {
                        Some(pt) => Some(self.wpool.alloc_node(
                            self.work,
                            self.core,
                            WorkTypeNode::Perm(pt),
                            span,
                        )?),
                        None => None,
                    };
                    if wcases
                        .push_work(
                            self.work,
                            self.core,
                            LocalVariantCase {
                                name: c.name,
                                payload,
                            },
                        )
                        .is_none()
                    {
                        return Err(oom(self.core, span));
                    }
                }
                Ok(LocalSchemaKind::Variant {
                    type_name: new_name,
                    cases: wcases,
                })
            }
        }
    }

    fn preregister_defn(
        &mut self,
        d: &crate::ast::DefnExpr,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        self.check_no_val_redef(d.name.as_str(), span)?;
        let mut params = RawVec::<(RawStr, WorkTypeId)>::new();
        let mut param_tys = RawVec::new();
        for p in d.params.as_slice() {
            for &(prev, _) in params.as_slice() {
                if prev.as_str() == p.name.as_str() {
                    return Err(self.type_err("expr.defn", span, "duplicate parameter name"));
                }
            }
            let pty = match p.ty {
                Some(tb) => self.resolve_type_expr(tb.as_ref())?,
                None if !self.opts.strict_defn_annotations => {
                    self.wpool.fresh_var(self.work, self.core, span)?
                }
                None => {
                    return Err(self.type_err(
                        "expr.defn",
                        span,
                        "defn parameter requires type annotation",
                    ));
                }
            };
            if params
                .push_work(self.work, self.core, (p.name, pty))
                .is_none()
                || param_tys.push_work(self.work, self.core, pty).is_none()
            {
                return Err(oom(self.core, span));
            }
        }
        let ret = match d.ret_ty {
            Some(rb) => self.resolve_type_expr(rb.as_ref())?,
            None if !self.opts.strict_defn_annotations => {
                self.wpool.fresh_var(self.work, self.core, span)?
            }
            None => {
                return Err(self.type_err(
                    "expr.defn",
                    span,
                    "defn requires return type annotation '-> Type'",
                ));
            }
        };
        let fn_ty = self.wpool.alloc_node(
            self.work,
            self.core,
            WorkTypeNode::Fn {
                params: param_tys,
                ret,
            },
            span,
        )?;
        let h = fnv1a_bytes(d.name.as_bytes());
        if self
            .local_val_map
            .insert_work(self.work, self.core, h, d.name, fn_ty, |a, b| {
                a.as_str() == b.as_str()
            })
            .is_none()
        {
            return Err(oom(self.core, span));
        }
        if self
            .local_defns
            .push_work(
                self.work,
                self.core,
                LocalDefnInfo {
                    name: d.name,
                    span,
                    params,
                    ret,
                    fn_ty,
                },
            )
            .is_none()
        {
            return Err(oom(self.core, span));
        }
        Ok(())
    }

    fn check_defn_body(
        &mut self,
        d: &crate::ast::DefnExpr,
        info: LocalDefnInfo,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        let saved = self.scopes.len();
        for &(pname, pty) in info.params.as_slice() {
            if self
                .scopes
                .push_work(
                    self.work,
                    self.core,
                    ScopeBinding {
                        name: pname,
                        ty: pty,
                    },
                )
                .is_none()
            {
                return Err(oom(self.core, span));
            }
        }
        let body = d.body.as_slice();
        let res = (|| -> Result<(), ErrRecorded> {
            for e in &body[..body.len().saturating_sub(1)] {
                let _ = self.check_expr(e, None)?;
            }
            if let Some(last) = body.last() {
                let actual = self.check_expr(last, Some(info.ret))?;
                let _ = self.wpool.coerce_to_expected(
                    actual,
                    info.ret,
                    self.env,
                    self.work,
                    self.core,
                    last.span,
                    "expr.defn",
                )?;
            }
            Ok(())
        })();
        self.scopes.truncate(saved);
        res
    }

    fn check_no_val_redef(&mut self, name: &str, span: Span) -> Result<(), ErrRecorded> {
        let h = fnv1a_bytes(name.as_bytes());
        if self
            .local_val_map
            .get_by(h, |k| k.as_str() == name)
            .is_some()
        {
            return Err(self.core.err_str(
                "expr.def",
                ErrorKind::Runtime,
                span,
                "cannot redefine name in same unit",
            ));
        }
        if self.opts.module_name.is_none() && self.env.lookup_root_val(name).is_some() {
            return Err(self.core.err_str(
                "expr.def",
                ErrorKind::Runtime,
                span,
                "cannot redefine existing global name",
            ));
        }
        Ok(())
    }

    fn lookup_schema_ref(
        &mut self,
        module: Option<RawStr>,
        name: RawStr,
        span: Span,
    ) -> Result<WorkSchemaId, ErrRecorded> {
        if let Some(mname) = module {
            let mh = fnv1a_bytes(mname.as_bytes());
            let mod_slot = self
                .imported_modules
                .get_by(mh, |k| k.as_str() == mname.as_str())
                .or_else(|| {
                    if self.opts.frozen {
                        self.env.lookup_module(mname.as_str()).map(|(idx, _)| idx)
                    } else {
                        None
                    }
                });
            let Some(slot_idx) = mod_slot else {
                return Err(self.type_err("expr.module", span, "module is not imported"));
            };
            let Some(menv) = self.env.get_module(slot_idx) else {
                return Err(self.type_err("expr.module", span, "unknown module"));
            };
            let th = fnv1a_bytes(name.as_bytes());
            let Some(sid) = menv
                .exports_types
                .get_by(th, |k| k.as_str() == name.as_str())
            else {
                return Err(self.type_err("expr.module", span, "module does not export this type"));
            };
            self.add_perm_schema_budget(sid);
            return Ok(WorkSchemaId::Perm(sid));
        }
        let h = fnv1a_bytes(name.as_bytes());
        if let Some(lidx) = self
            .local_type_map
            .get_by(h, |k| k.as_str() == name.as_str())
        {
            return Ok(WorkSchemaId::Local(lidx));
        }
        if self.opts.module_name.is_none()
            && let Some(sid) = self.env.lookup_root_type(name.as_str())
        {
            self.add_perm_schema_budget(sid);
            return Ok(WorkSchemaId::Perm(sid));
        }
        Err(self.type_err("type.defschema", span, "unknown type name"))
    }

    fn add_perm_schema_budget(&mut self, sid: super::types::SchemaId) {
        let items = match self.env.get_schema(sid).kind {
            SchemaKind::Struct { fields, .. } => fields.len(),
            SchemaKind::Variant { cases, .. } => cases.len(),
        };
        self.wpool.budget_remaining = self
            .wpool
            .budget_remaining
            .saturating_add(items.saturating_mul(TYPECHECK_WORK_MULTIPLIER));
    }

    pub(super) fn resolve_type_expr(&mut self, te: &TypeExpr) -> Result<WorkTypeId, ErrRecorded> {
        self.wpool.tick(self.core, te.span)?;
        match &te.kind {
            TypeExprKind::Void => Ok(WTY_VOID),
            TypeExprKind::I64 => Ok(WTY_I64),
            TypeExprKind::F64 => Ok(WTY_F64),
            TypeExprKind::Bool => Ok(WTY_BOOL),
            TypeExprKind::Char => Ok(WTY_CHAR),
            TypeExprKind::Text => Ok(WTY_TEXT),
            TypeExprKind::Ident => Ok(WTY_IDENT),
            TypeExprKind::Datum => Ok(WTY_DATUM),
            TypeExprKind::List(inner) => {
                let t = self.resolve_type_expr(inner.as_ref())?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(t), te.span)
            }
            TypeExprKind::Array(inner) => {
                let t = self.resolve_type_expr(inner.as_ref())?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::Array(t), te.span)
            }
            TypeExprKind::Option(inner) => {
                let t = self.resolve_type_expr(inner.as_ref())?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::Option(t), te.span)
            }
            TypeExprKind::Result { ok, err } => {
                let to = self.resolve_type_expr(ok.as_ref())?;
                let te_id = self.resolve_type_expr(err.as_ref())?;
                self.wpool.alloc_node(
                    self.work,
                    self.core,
                    WorkTypeNode::Result { ok: to, err: te_id },
                    te.span,
                )
            }
            TypeExprKind::Tuple(items) => {
                let mut out = RawVec::new();
                for it in items.as_slice() {
                    let t = self.resolve_type_expr(it)?;
                    if out.push_work(self.work, self.core, t).is_none() {
                        return Err(oom(self.core, te.span));
                    }
                }
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::Tuple(out), te.span)
            }
            TypeExprKind::Fn { params, ret } => {
                let mut out = RawVec::new();
                for p in params.as_slice() {
                    let t = self.resolve_type_expr(p)?;
                    if out.push_work(self.work, self.core, t).is_none() {
                        return Err(oom(self.core, te.span));
                    }
                }
                let tr = self.resolve_type_expr(ret.as_ref())?;
                self.wpool.alloc_node(
                    self.work,
                    self.core,
                    WorkTypeNode::Fn {
                        params: out,
                        ret: tr,
                    },
                    te.span,
                )
            }
            TypeExprKind::Named { module, name } => {
                let sid = self.lookup_schema_ref(*module, *name, te.span)?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::Schema(sid), te.span)
            }
        }
    }

    /// 双方向型検査の本体。`expected` があれば内側へ期待型を伝播する。
    pub(super) fn check_expr(
        &mut self,
        expr: &Expr,
        expected: Option<WorkTypeId>,
    ) -> Result<WorkTypeId, ErrRecorded> {
        self.wpool.tick(self.core, expr.span)?;
        let span = expr.span;
        match &expr.kind {
            ExprKind::Int(_) => Ok(WTY_I64),
            ExprKind::Float(_) => Ok(WTY_F64),
            ExprKind::Bool(_) => Ok(WTY_BOOL),
            ExprKind::Char(_) => Ok(WTY_CHAR),
            ExprKind::Str(_) => Ok(WTY_TEXT),
            ExprKind::None => {
                if let Some(exp) = expected {
                    let er = self.wpool.resolve(exp, self.core, span)?;
                    if matches!(self.wpool.get(er), WorkTypeNode::Perm(TY_DATUM)) {
                        return Ok(WTY_DATUM);
                    }
                    if matches!(
                        self.wpool
                            .unwrap_perm(er, self.env, self.work, self.core, span)?,
                        WorkTypeNode::Option(_)
                    ) {
                        return Ok(er);
                    }
                }
                let elem = self.wpool.fresh_var(self.work, self.core, span)?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::Option(elem), span)
            }
            ExprKind::Ident(name) => self.lookup_value_name(name.as_str(), span),
            ExprKind::Template(parts) => {
                for part in parts.as_slice() {
                    let pt = self.check_expr(part, None)?;
                    if !self.wpool.is_basic6(pt, self.core, part.span)? {
                        return Err(self.type_err(
                            "expr.template",
                            part.span,
                            "template interpolation expects i64, f64, bool, char, text, or ident",
                        ));
                    }
                }
                Ok(WTY_TEXT)
            }
            ExprKind::Quote(datum) => match datum.as_ref().kind {
                DatumKind::Ident(_) => Ok(WTY_IDENT),
                _ => Ok(WTY_DATUM),
            },
            ExprKind::Quasiquote(datum) => {
                self.check_quasiquote_datum(datum.as_ref())?;
                Ok(WTY_DATUM)
            }
            ExprKind::List(items) => self.check_list_elements(items.as_slice(), expected, span),
            ExprKind::Tuple(items) => {
                let exp_items = match expected {
                    Some(e) => match self
                        .wpool
                        .unwrap_perm(e, self.env, self.work, self.core, span)?
                    {
                        WorkTypeNode::Tuple(ts) if ts.len() == items.len() => Some(ts),
                        _ => None,
                    },
                    None => None,
                };
                let mut out = RawVec::new();
                for (i, item) in items.as_slice().iter().enumerate() {
                    let exp_i = exp_items.map(|ts| ts.as_slice()[i]);
                    let ti = self.check_expr(item, exp_i)?;
                    let ti = match exp_i {
                        Some(ei) => self.wpool.coerce_to_expected(
                            ti,
                            ei,
                            self.env,
                            self.work,
                            self.core,
                            item.span,
                            "expr.tuple",
                        )?,
                        None => ti,
                    };
                    if out.push_work(self.work, self.core, ti).is_none() {
                        return Err(oom(self.core, span));
                    }
                }
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::Tuple(out), span)
            }
            ExprKind::Struct(s) => {
                let s = s.as_ref();
                let sid = self.lookup_schema_ref(None, s.name, span)?;
                self.check_struct_literal(sid, s.fields.as_slice(), span)
            }
            ExprKind::Variant(v) => {
                let v = v.as_ref();
                let sid = self.lookup_schema_ref(None, v.type_name, span)?;
                self.check_variant_literal(
                    sid,
                    v.variant,
                    v.data.as_ref().map(|b| b.as_ref()),
                    span,
                )
            }
            ExprKind::Some(inner) => {
                let exp_inner = match expected {
                    Some(e) => match self
                        .wpool
                        .unwrap_perm(e, self.env, self.work, self.core, span)?
                    {
                        WorkTypeNode::Option(t) => Some(t),
                        _ => None,
                    },
                    None => None,
                };
                let ti = self.check_expr(inner.as_ref(), exp_inner)?;
                let ti = match exp_inner {
                    Some(ei) => self.wpool.coerce_to_expected(
                        ti,
                        ei,
                        self.env,
                        self.work,
                        self.core,
                        inner.span,
                        "type.builtin",
                    )?,
                    None => ti,
                };
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::Option(ti), span)
            }
            ExprKind::Ok(inner) => {
                let (exp_ok, exp_err) = match expected {
                    Some(e) => match self
                        .wpool
                        .unwrap_perm(e, self.env, self.work, self.core, span)?
                    {
                        WorkTypeNode::Result { ok, err } => (Some(ok), Some(err)),
                        _ => (None, None),
                    },
                    None => (None, None),
                };
                let tok = self.check_expr(inner.as_ref(), exp_ok)?;
                let tok = match exp_ok {
                    Some(eo) => self.wpool.coerce_to_expected(
                        tok,
                        eo,
                        self.env,
                        self.work,
                        self.core,
                        inner.span,
                        "type.builtin",
                    )?,
                    None => tok,
                };
                let terr = match exp_err {
                    Some(ee) => ee,
                    None => self.wpool.fresh_var(self.work, self.core, span)?,
                };
                self.wpool.alloc_node(
                    self.work,
                    self.core,
                    WorkTypeNode::Result { ok: tok, err: terr },
                    span,
                )
            }
            ExprKind::Err(inner) => {
                let (exp_ok, exp_err) = match expected {
                    Some(e) => match self
                        .wpool
                        .unwrap_perm(e, self.env, self.work, self.core, span)?
                    {
                        WorkTypeNode::Result { ok, err } => (Some(ok), Some(err)),
                        _ => (None, None),
                    },
                    None => (None, None),
                };
                let terr = self.check_expr(inner.as_ref(), exp_err)?;
                let terr = match exp_err {
                    Some(ee) => self.wpool.coerce_to_expected(
                        terr,
                        ee,
                        self.env,
                        self.work,
                        self.core,
                        inner.span,
                        "type.builtin",
                    )?,
                    None => terr,
                };
                let tok = match exp_ok {
                    Some(eo) => eo,
                    None => self.wpool.fresh_var(self.work, self.core, span)?,
                };
                self.wpool.alloc_node(
                    self.work,
                    self.core,
                    WorkTypeNode::Result { ok: tok, err: terr },
                    span,
                )
            }
            ExprKind::Field { target, field } => {
                let tt = self.check_expr(target.as_ref(), None)?;
                let node =
                    self.wpool
                        .unwrap_perm(tt, self.env, self.work, self.core, target.span)?;
                match node {
                    WorkTypeNode::Module(slot_idx) => {
                        let Some(menv) = self.env.get_module(slot_idx) else {
                            return Err(self.type_err("expr.module", span, "unknown module"));
                        };
                        let h = fnv1a_bytes(field.as_bytes());
                        let Some(vb) = menv
                            .exports_vals
                            .get_by(h, |k| k.as_str() == field.as_str())
                        else {
                            return Err(self.type_err(
                                "expr.module",
                                span,
                                "module does not export this symbol",
                            ));
                        };
                        self.wpool
                            .alloc_node(self.work, self.core, WorkTypeNode::Perm(vb.ty), span)
                    }
                    WorkTypeNode::Schema(sid) => {
                        self.lookup_struct_field_type(sid, field.as_str(), span, "expr.field")
                    }
                    _ => Err(self.type_err(
                        "expr.field",
                        span,
                        "field access requires a struct or module",
                    )),
                }
            }
            ExprKind::Index { target, index } => {
                let tt = self.check_expr(target.as_ref(), None)?;
                let ti = self.check_expr(index.as_ref(), Some(WTY_I64))?;
                self.wpool.unify(
                    ti,
                    WTY_I64,
                    self.env,
                    self.work,
                    self.core,
                    index.span,
                    "expr.index",
                )?;
                match self
                    .wpool
                    .unwrap_perm(tt, self.env, self.work, self.core, target.span)?
                {
                    WorkTypeNode::List(elem) | WorkTypeNode::Array(elem) => Ok(elem),
                    WorkTypeNode::Perm(TY_TEXT) => Ok(WTY_CHAR),
                    _ => Err(self.type_err(
                        "expr.index",
                        target.span,
                        "indexing requires list, array, or text",
                    )),
                }
            }
            ExprKind::With { target, fields } => {
                let tt = self.check_expr(target.as_ref(), expected)?;
                let WorkTypeNode::Schema(sid) =
                    self.wpool
                        .unwrap_perm(tt, self.env, self.work, self.core, target.span)?
                else {
                    return Err(self.type_err(
                        "expr.with",
                        target.span,
                        "'with' requires a struct",
                    ));
                };
                for &(fname, ref fexpr) in fields.as_slice() {
                    let fty = self.lookup_struct_field_type(
                        sid,
                        fname.as_str(),
                        fexpr.span,
                        "expr.with",
                    )?;
                    let actual = self.check_expr(fexpr, Some(fty))?;
                    let _ = self.wpool.coerce_to_expected(
                        actual,
                        fty,
                        self.env,
                        self.work,
                        self.core,
                        fexpr.span,
                        "expr.with",
                    )?;
                }
                Ok(tt)
            }
            ExprKind::Let(l) => {
                let l = l.as_ref();
                let saved = self.scopes.len();
                let res = (|| -> Result<WorkTypeId, ErrRecorded> {
                    for &(bname, ref bexpr) in l.bindings.as_slice() {
                        let bty = self.check_expr(bexpr, None)?;
                        if self
                            .scopes
                            .push_work(
                                self.work,
                                self.core,
                                ScopeBinding {
                                    name: bname,
                                    ty: bty,
                                },
                            )
                            .is_none()
                        {
                            return Err(oom(self.core, bexpr.span));
                        }
                    }
                    let body = l.body.as_slice();
                    for e in &body[..body.len().saturating_sub(1)] {
                        let _ = self.check_expr(e, None)?;
                    }
                    match body.last() {
                        Some(last) => self.check_expr(last, expected),
                        None => Ok(WTY_VOID),
                    }
                })();
                self.scopes.truncate(saved);
                res
            }
            ExprKind::Fn(f) => {
                let f = f.as_ref();
                let (exp_params, exp_ret) = match expected {
                    Some(e) => match self
                        .wpool
                        .unwrap_perm(e, self.env, self.work, self.core, span)?
                    {
                        WorkTypeNode::Fn { params, ret } if params.len() == f.params.len() => {
                            (Some(params), Some(ret))
                        }
                        _ => (None, None),
                    },
                    None => (None, None),
                };
                let saved = self.scopes.len();
                let res = (|| -> Result<WorkTypeId, ErrRecorded> {
                    let mut p_tys = RawVec::new();
                    for (i, p) in f.params.as_slice().iter().enumerate() {
                        let exp_pi = exp_params.map(|ps| ps.as_slice()[i]);
                        let pty = match (p.ty, exp_pi) {
                            (Some(tb), Some(ep)) => {
                                let t = self.resolve_type_expr(tb.as_ref())?;
                                self.wpool.unify(
                                    t, ep, self.env, self.work, self.core, tb.span, "expr.fn",
                                )?;
                                t
                            }
                            (Some(tb), None) => self.resolve_type_expr(tb.as_ref())?,
                            (None, Some(ep)) => ep,
                            (None, None) => self.wpool.fresh_var(self.work, self.core, span)?,
                        };
                        if p_tys.push_work(self.work, self.core, pty).is_none()
                            || self
                                .scopes
                                .push_work(
                                    self.work,
                                    self.core,
                                    ScopeBinding {
                                        name: p.name,
                                        ty: pty,
                                    },
                                )
                                .is_none()
                        {
                            return Err(oom(self.core, span));
                        }
                    }
                    let ret_exp = match (f.ret_ty, exp_ret) {
                        (Some(rb), Some(er)) => {
                            let t = self.resolve_type_expr(rb.as_ref())?;
                            self.wpool
                                .unify(t, er, self.env, self.work, self.core, rb.span, "expr.fn")?;
                            Some(t)
                        }
                        (Some(rb), None) => Some(self.resolve_type_expr(rb.as_ref())?),
                        (None, Some(er)) => Some(er),
                        (None, None) => None,
                    };
                    let body = f.body.as_slice();
                    for e in &body[..body.len().saturating_sub(1)] {
                        let _ = self.check_expr(e, None)?;
                    }
                    let body_ty = match body.last() {
                        Some(last) => self.check_expr(last, ret_exp)?,
                        None => WTY_VOID,
                    };
                    let final_ret = match ret_exp {
                        Some(re) => self.wpool.coerce_to_expected(
                            body_ty, re, self.env, self.work, self.core, span, "expr.fn",
                        )?,
                        None => body_ty,
                    };
                    self.wpool.alloc_node(
                        self.work,
                        self.core,
                        WorkTypeNode::Fn {
                            params: p_tys,
                            ret: final_ret,
                        },
                        span,
                    )
                })();
                self.scopes.truncate(saved);
                res
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let tc = self.check_expr(cond.as_ref(), Some(WTY_BOOL))?;
                self.wpool.unify(
                    tc,
                    WTY_BOOL,
                    self.env,
                    self.work,
                    self.core,
                    cond.span,
                    "expr.condition",
                )?;
                let tt = self.check_expr(then_branch.as_ref(), expected)?;
                let te = self.check_expr(else_branch.as_ref(), expected.or(Some(tt)))?;
                match expected {
                    Some(exp) => {
                        let _ = self.wpool.coerce_to_expected(
                            tt,
                            exp,
                            self.env,
                            self.work,
                            self.core,
                            then_branch.span,
                            "expr.if",
                        )?;
                        self.wpool.coerce_to_expected(
                            te,
                            exp,
                            self.env,
                            self.work,
                            self.core,
                            else_branch.span,
                            "expr.if",
                        )
                    }
                    None => {
                        self.wpool
                            .unify(tt, te, self.env, self.work, self.core, span, "expr.if")?;
                        Ok(tt)
                    }
                }
            }
            ExprKind::When { cond, body } => {
                let tc = self.check_expr(cond.as_ref(), Some(WTY_BOOL))?;
                self.wpool.unify(
                    tc,
                    WTY_BOOL,
                    self.env,
                    self.work,
                    self.core,
                    cond.span,
                    "expr.condition",
                )?;
                for e in body.as_slice() {
                    let _ = self.check_expr(e, None)?;
                }
                Ok(WTY_VOID)
            }
            ExprKind::Match { scrutinee, arms } => {
                self.check_match(scrutinee.as_ref(), arms.as_slice(), expected, span)
            }
            ExprKind::Call { callee, args } => {
                // テストモードの特殊呼び出し (`deftest`, `check`, `check_eq`)
                if self.opts.test_mode
                    && let ExprKind::Ident(cname) = callee.as_ref().kind
                    && matches!(cname.as_str(), "deftest" | "check" | "check_eq")
                {
                    return self.check_test_builtin_call(cname.as_str(), args.as_slice(), span);
                }
                let ct = self.check_expr(callee.as_ref(), None)?;
                let cn = self
                    .wpool
                    .unwrap_perm(ct, self.env, self.work, self.core, callee.span)?;
                match cn {
                    WorkTypeNode::Builtin(b) => {
                        self.check_builtin_call(b, args.as_slice(), expected, span)
                    }
                    WorkTypeNode::Fn { params, ret } => {
                        if params.len() != args.len() {
                            return Err(self.type_err(
                                "expr.call",
                                span,
                                "wrong number of arguments",
                            ));
                        }
                        for (arg, &pty) in args.as_slice().iter().zip(params.as_slice()) {
                            let at = self.check_expr(arg, Some(pty))?;
                            let ar = self.wpool.resolve(at, self.core, arg.span)?;
                            if let WorkTypeNode::Builtin(b) = self.wpool.get(ar) {
                                self.instantiate_builtin_as_fn(b, pty, arg.span, "expr.call")?;
                            } else {
                                let _ = self.wpool.coerce_to_expected(
                                    ar,
                                    pty,
                                    self.env,
                                    self.work,
                                    self.core,
                                    arg.span,
                                    "expr.call",
                                )?;
                            }
                        }
                        Ok(ret)
                    }
                    WorkTypeNode::Var(_) => {
                        let mut p_tys = RawVec::new();
                        for arg in args.as_slice() {
                            let at = self.check_expr(arg, None)?;
                            if p_tys.push_work(self.work, self.core, at).is_none() {
                                return Err(oom(self.core, arg.span));
                            }
                        }
                        let ret = match expected {
                            Some(e) => e,
                            None => self.wpool.fresh_var(self.work, self.core, span)?,
                        };
                        let fn_ty = self.wpool.alloc_node(
                            self.work,
                            self.core,
                            WorkTypeNode::Fn { params: p_tys, ret },
                            span,
                        )?;
                        self.wpool.unify(
                            ct,
                            fn_ty,
                            self.env,
                            self.work,
                            self.core,
                            span,
                            "expr.call",
                        )?;
                        Ok(ret)
                    }
                    _ => Err(self.type_err("expr.call", callee.span, "cannot call non-function")),
                }
            }
            ExprKind::Def(_) => {
                Err(self.type_err("expr.def", span, "def is only allowed at top level"))
            }
            ExprKind::DefSchema(_) => Err(self.type_err(
                "type.defschema",
                span,
                "defschema is only allowed at top level",
            )),
            ExprKind::Defn(_) => {
                Err(self.type_err("expr.defn", span, "defn is only allowed at top level"))
            }
            ExprKind::Import { .. } | ExprKind::Export { .. } => Err(self.type_err(
                "expr.module",
                span,
                "import/export is only allowed at top level",
            )),
        }
    }

    pub(super) fn check_list_elements(
        &mut self,
        items: &[Expr],
        expected: Option<WorkTypeId>,
        span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        let elem_ty = match self.expected_list_elem(expected, span)? {
            Some(e) => e,
            None => self.wpool.fresh_var(self.work, self.core, span)?,
        };
        for item in items {
            let it = self.check_expr(item, Some(elem_ty))?;
            let _ = self.wpool.coerce_to_expected(
                it,
                elem_ty,
                self.env,
                self.work,
                self.core,
                item.span,
                "expr.list",
            )?;
        }
        self.wpool
            .alloc_node(self.work, self.core, WorkTypeNode::List(elem_ty), span)
    }

    fn check_quasiquote_datum(&mut self, root: &Datum) -> Result<(), ErrRecorded> {
        let list_datum = self.wpool.alloc_node(
            self.work,
            self.core,
            WorkTypeNode::List(WTY_DATUM),
            root.span,
        )?;
        let mut stack = RawVec::<&Datum>::new();
        if stack.push_work(self.work, self.core, root).is_none() {
            return Err(oom(self.core, root.span));
        }
        while let Some(d) = stack.pop() {
            self.wpool.tick(self.core, d.span)?;
            match &d.kind {
                DatumKind::List(items) => {
                    for it in items.as_slice() {
                        if stack.push_work(self.work, self.core, it).is_none() {
                            return Err(oom(self.core, it.span));
                        }
                    }
                }
                DatumKind::Unquote(expr) => {
                    let et = self.check_expr(expr.as_ref(), Some(WTY_DATUM))?;
                    let _ = self.wpool.coerce_to_expected(
                        et,
                        WTY_DATUM,
                        self.env,
                        self.work,
                        self.core,
                        expr.span,
                        "expr.quasiquote",
                    )?;
                }
                DatumKind::UnquoteSplicing(expr) => {
                    let et = self.check_expr(expr.as_ref(), Some(list_datum))?;
                    let _ = self.wpool.coerce_to_expected(
                        et,
                        list_datum,
                        self.env,
                        self.work,
                        self.core,
                        expr.span,
                        "expr.quasiquote",
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn lookup_value_name(&mut self, name: &str, span: Span) -> Result<WorkTypeId, ErrRecorded> {
        // 1. ローカルスコープ（内側から）
        for b in self.scopes.as_slice().iter().rev() {
            if b.name.as_str() == name {
                return Ok(b.ty);
            }
        }
        // 2. 同一評価単位の def / defn
        let h = fnv1a_bytes(name.as_bytes());
        if let Some(ty) = self.local_val_map.get_by(h, |k| k.as_str() == name) {
            return Ok(ty);
        }
        // 3. 永続環境の大域束縛（モジュール内部では参照しない）
        if self.opts.module_name.is_none()
            && let Some(vb) = self.env.lookup_root_val(name)
        {
            return self
                .wpool
                .alloc_node(self.work, self.core, WorkTypeNode::Perm(vb.ty), span);
        }
        // 4. 組み込み定数 `pi`
        if name == "pi" {
            return Ok(WTY_F64);
        }
        // 5. 組み込み関数
        if let Some(b) = lookup_builtin_by_name(name) {
            return self
                .wpool
                .alloc_node(self.work, self.core, WorkTypeNode::Builtin(b), span);
        }
        // 6. import 済みモジュール（凍結環境では読み込み済み全モジュールも参照可）
        if let Some(slot_idx) = self.imported_modules.get_by(h, |k| k.as_str() == name) {
            return self.wpool.alloc_node(
                self.work,
                self.core,
                WorkTypeNode::Module(slot_idx),
                span,
            );
        }
        if self.opts.frozen
            && let Some((slot_idx, _)) = self.env.lookup_module(name)
        {
            return self.wpool.alloc_node(
                self.work,
                self.core,
                WorkTypeNode::Module(slot_idx),
                span,
            );
        }
        Err(self.type_err("expr.name", span, "undefined variable"))
    }

    fn lookup_struct_field_type(
        &mut self,
        sid: WorkSchemaId,
        field: &str,
        span: Span,
        rule: &'static str,
    ) -> Result<WorkTypeId, ErrRecorded> {
        match sid {
            WorkSchemaId::Perm(pid) => {
                let def = *self.env.get_schema(pid);
                let SchemaKind::Struct { fields, .. } = def.kind else {
                    return Err(self.type_err(rule, span, "target is not a struct"));
                };
                for &(fname, fty) in fields.as_slice() {
                    if fname.as_str() == field {
                        return self.wpool.alloc_node(
                            self.work,
                            self.core,
                            WorkTypeNode::Perm(fty),
                            span,
                        );
                    }
                }
                Err(self.type_err(rule, span, "unknown struct field"))
            }
            WorkSchemaId::Local(lidx) => {
                let kind = self.wpool.local_schemas.as_slice()[lidx as usize]
                    .kind
                    .expect("local schema resolved");
                let LocalSchemaKind::Struct { fields, .. } = kind else {
                    return Err(self.type_err(rule, span, "target is not a struct"));
                };
                for &(fname, fty) in fields.as_slice() {
                    if fname.as_str() == field {
                        return Ok(fty);
                    }
                }
                Err(self.type_err(rule, span, "unknown struct field"))
            }
        }
    }

    fn check_struct_literal(
        &mut self,
        sid: WorkSchemaId,
        provided: &[(RawStr, Expr)],
        span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        let mut expected_fields = RawVec::<(RawStr, WorkTypeId)>::new();
        match sid {
            WorkSchemaId::Perm(pid) => {
                let def = *self.env.get_schema(pid);
                let SchemaKind::Struct { fields, .. } = def.kind else {
                    return Err(self.type_err("expr.field", span, "type is not a struct"));
                };
                for &(fname, fty) in fields.as_slice() {
                    let wt = self.wpool.alloc_node(
                        self.work,
                        self.core,
                        WorkTypeNode::Perm(fty),
                        span,
                    )?;
                    if expected_fields
                        .push_work(self.work, self.core, (fname, wt))
                        .is_none()
                    {
                        return Err(oom(self.core, span));
                    }
                }
            }
            WorkSchemaId::Local(lidx) => {
                let kind = self.wpool.local_schemas.as_slice()[lidx as usize]
                    .kind
                    .expect("local schema resolved");
                let LocalSchemaKind::Struct { fields, .. } = kind else {
                    return Err(self.type_err("expr.field", span, "type is not a struct"));
                };
                for &(fname, fty) in fields.as_slice() {
                    if expected_fields
                        .push_work(self.work, self.core, (fname, fty))
                        .is_none()
                    {
                        return Err(oom(self.core, span));
                    }
                }
            }
        }

        let mut seen = RawVec::<bool>::new();
        for _ in 0..expected_fields.len() {
            if seen.push_work(self.work, self.core, false).is_none() {
                return Err(oom(self.core, span));
            }
        }

        for (fname, fexpr) in provided {
            let mut found = None;
            for (idx, &(efn, efty)) in expected_fields.as_slice().iter().enumerate() {
                if efn.as_str() == fname.as_str() {
                    found = Some((idx, efty));
                    break;
                }
            }
            let Some((idx, efty)) = found else {
                return Err(self.type_err("expr.field", fexpr.span, "unknown struct field"));
            };
            if seen.as_slice()[idx] {
                return Err(self.type_err("expr.field", fexpr.span, "duplicate struct field"));
            }
            seen.as_mut_slice()[idx] = true;
            let actual = self.check_expr(fexpr, Some(efty))?;
            let _ = self.wpool.coerce_to_expected(
                actual,
                efty,
                self.env,
                self.work,
                self.core,
                fexpr.span,
                "expr.field",
            )?;
        }

        for &s in seen.as_slice() {
            if !s {
                return Err(self.type_err("expr.field", span, "missing required struct field"));
            }
        }

        self.wpool
            .alloc_node(self.work, self.core, WorkTypeNode::Schema(sid), span)
    }

    fn check_variant_literal(
        &mut self,
        sid: WorkSchemaId,
        variant: RawStr,
        data: Option<&Expr>,
        span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        let payload_ty = self.lookup_variant_case(sid, variant.as_str(), span)?.1;
        match (payload_ty, data) {
            (Some(pty), Some(dexpr)) => {
                let actual = self.check_expr(dexpr, Some(pty))?;
                let _ = self.wpool.coerce_to_expected(
                    actual,
                    pty,
                    self.env,
                    self.work,
                    self.core,
                    dexpr.span,
                    "expr.variant",
                )?;
            }
            (None, None) => {}
            (Some(_), None) => {
                return Err(self.type_err("expr.variant", span, "variant case requires a payload"));
            }
            (None, Some(dexpr)) => {
                return Err(self.type_err(
                    "expr.variant",
                    dexpr.span,
                    "variant case does not take a payload",
                ));
            }
        }
        self.wpool
            .alloc_node(self.work, self.core, WorkTypeNode::Schema(sid), span)
    }

    fn lookup_variant_case(
        &mut self,
        sid: WorkSchemaId,
        variant: &str,
        span: Span,
    ) -> Result<(usize, Option<WorkTypeId>), ErrRecorded> {
        match sid {
            WorkSchemaId::Perm(pid) => {
                let def = *self.env.get_schema(pid);
                let SchemaKind::Variant { cases, .. } = def.kind else {
                    return Err(self.type_err("expr.variant", span, "type is not a variant"));
                };
                for (i, &c) in cases.as_slice().iter().enumerate() {
                    if c.name.as_str() == variant {
                        let pty = match c.payload {
                            Some(pt) => Some(self.wpool.alloc_node(
                                self.work,
                                self.core,
                                WorkTypeNode::Perm(pt),
                                span,
                            )?),
                            None => None,
                        };
                        return Ok((i, pty));
                    }
                }
                Err(self.type_err("expr.variant", span, "unknown variant case"))
            }
            WorkSchemaId::Local(lidx) => {
                let kind = self.wpool.local_schemas.as_slice()[lidx as usize]
                    .kind
                    .expect("local schema resolved");
                let LocalSchemaKind::Variant { cases, .. } = kind else {
                    return Err(self.type_err("expr.variant", span, "type is not a variant"));
                };
                for (i, &c) in cases.as_slice().iter().enumerate() {
                    if c.name.as_str() == variant {
                        return Ok((i, c.payload));
                    }
                }
                Err(self.type_err("expr.variant", span, "unknown variant case"))
            }
        }
    }

    fn variant_case_count(&self, sid: WorkSchemaId) -> Option<usize> {
        match sid {
            WorkSchemaId::Perm(pid) => match self.env.get_schema(pid).kind {
                SchemaKind::Variant { cases, .. } => Some(cases.len()),
                SchemaKind::Struct { .. } => None,
            },
            WorkSchemaId::Local(lidx) => {
                match self.wpool.local_schemas.as_slice()[lidx as usize].kind? {
                    LocalSchemaKind::Variant { cases, .. } => Some(cases.len()),
                    LocalSchemaKind::Struct { .. } => None,
                }
            }
        }
    }

    fn check_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        expected: Option<WorkTypeId>,
        span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        let scrut_ty = self.check_expr(scrutinee, None)?;
        let scrut_node =
            self.wpool
                .unwrap_perm(scrut_ty, self.env, self.work, self.core, scrutinee.span)?;

        // 網羅性・到達不能性追跡フラグ
        let mut caught_all = false;
        let mut seen_bool = [false; 2]; // [false, true]
        let mut seen_opt = [false; 2]; // [none, some]
        let mut seen_res = [false; 2]; // [ok, err]
        // datum 8 種: i64, f64, bool, char, text, ident, list, none
        let mut seen_datum = [false; 8];
        let mut seen_variant = RawVec::<bool>::new();
        if let WorkTypeNode::Schema(sid) = scrut_node
            && let Some(cnt) = self.variant_case_count(sid)
        {
            for _ in 0..cnt {
                if seen_variant
                    .push_work(self.work, self.core, false)
                    .is_none()
                {
                    return Err(oom(self.core, span));
                }
            }
        }

        let mut result_ty: Option<WorkTypeId> = expected;
        for arm in arms {
            if caught_all {
                return Err(self.type_err("expr.match", arm.span, "unreachable match arm"));
            }
            let saved = self.scopes.len();
            let arm_res = (|| -> Result<WorkTypeId, ErrRecorded> {
                match &arm.pattern {
                    Pattern::Wildcard => {
                        caught_all = true;
                    }
                    Pattern::Var(name) => {
                        caught_all = true;
                        if self
                            .scopes
                            .push_work(
                                self.work,
                                self.core,
                                ScopeBinding {
                                    name: *name,
                                    ty: scrut_ty,
                                },
                            )
                            .is_none()
                        {
                            return Err(oom(self.core, arm.span));
                        }
                    }
                    Pattern::Int(_) => {
                        let sn = self
                            .wpool
                            .unwrap_perm(scrut_ty, self.env, self.work, self.core, arm.span)?;
                        if matches!(sn, WorkTypeNode::Perm(TY_DATUM)) {
                            if seen_datum[0] {
                                return Err(self.type_err(
                                    "expr.match",
                                    arm.span,
                                    "unreachable match arm",
                                ));
                            }
                        } else {
                            self.wpool.unify(
                                scrut_ty,
                                WTY_I64,
                                self.env,
                                self.work,
                                self.core,
                                arm.span,
                                "expr.match",
                            )?;
                        }
                    }
                    Pattern::Bool(b) => {
                        let sn = self
                            .wpool
                            .unwrap_perm(scrut_ty, self.env, self.work, self.core, arm.span)?;
                        if matches!(sn, WorkTypeNode::Perm(TY_DATUM)) {
                            if seen_datum[2] {
                                return Err(self.type_err(
                                    "expr.match",
                                    arm.span,
                                    "unreachable match arm",
                                ));
                            }
                        } else {
                            self.wpool.unify(
                                scrut_ty,
                                WTY_BOOL,
                                self.env,
                                self.work,
                                self.core,
                                arm.span,
                                "expr.match",
                            )?;
                            let idx = usize::from(*b);
                            if seen_bool[idx] {
                                return Err(self.type_err(
                                    "expr.match",
                                    arm.span,
                                    "unreachable match arm",
                                ));
                            }
                            seen_bool[idx] = true;
                            if seen_bool[0] && seen_bool[1] {
                                caught_all = true;
                            }
                        }
                    }
                    Pattern::Char(_) => {
                        let sn = self
                            .wpool
                            .unwrap_perm(scrut_ty, self.env, self.work, self.core, arm.span)?;
                        if matches!(sn, WorkTypeNode::Perm(TY_DATUM)) {
                            if seen_datum[3] {
                                return Err(self.type_err(
                                    "expr.match",
                                    arm.span,
                                    "unreachable match arm",
                                ));
                            }
                        } else {
                            self.wpool.unify(
                                scrut_ty,
                                WTY_CHAR,
                                self.env,
                                self.work,
                                self.core,
                                arm.span,
                                "expr.match",
                            )?;
                        }
                    }
                    Pattern::Str(_) => {
                        let sn = self
                            .wpool
                            .unwrap_perm(scrut_ty, self.env, self.work, self.core, arm.span)?;
                        if matches!(sn, WorkTypeNode::Perm(TY_DATUM)) {
                            if seen_datum[4] {
                                return Err(self.type_err(
                                    "expr.match",
                                    arm.span,
                                    "unreachable match arm",
                                ));
                            }
                        } else {
                            self.wpool.unify(
                                scrut_ty,
                                WTY_TEXT,
                                self.env,
                                self.work,
                                self.core,
                                arm.span,
                                "expr.match",
                            )?;
                        }
                    }
                    Pattern::Ident(_) => {
                        let sn = self
                            .wpool
                            .unwrap_perm(scrut_ty, self.env, self.work, self.core, arm.span)?;
                        if matches!(sn, WorkTypeNode::Perm(TY_DATUM)) {
                            if seen_datum[5] {
                                return Err(self.type_err(
                                    "expr.match",
                                    arm.span,
                                    "unreachable match arm",
                                ));
                            }
                        } else {
                            self.wpool.unify(
                                scrut_ty,
                                WTY_IDENT,
                                self.env,
                                self.work,
                                self.core,
                                arm.span,
                                "expr.match",
                            )?;
                        }
                    }
                    Pattern::None => {
                        let sn = self
                            .wpool
                            .unwrap_perm(scrut_ty, self.env, self.work, self.core, arm.span)?;
                        if matches!(sn, WorkTypeNode::Perm(TY_DATUM)) {
                            if seen_datum[7] {
                                return Err(self.type_err(
                                    "expr.match",
                                    arm.span,
                                    "unreachable match arm",
                                ));
                            }
                            seen_datum[7] = true;
                            if seen_datum.iter().all(|&x| x) {
                                caught_all = true;
                            }
                        } else {
                            let elem = self.wpool.fresh_var(self.work, self.core, arm.span)?;
                            let opt = self.wpool.alloc_node(
                                self.work,
                                self.core,
                                WorkTypeNode::Option(elem),
                                arm.span,
                            )?;
                            self.wpool.unify(
                                scrut_ty,
                                opt,
                                self.env,
                                self.work,
                                self.core,
                                arm.span,
                                "expr.match",
                            )?;
                            if seen_opt[0] {
                                return Err(self.type_err(
                                    "expr.match",
                                    arm.span,
                                    "unreachable match arm",
                                ));
                            }
                            seen_opt[0] = true;
                            if seen_opt[0] && seen_opt[1] {
                                caught_all = true;
                            }
                        }
                    }
                    Pattern::Some(binding) => {
                        let elem = self.wpool.fresh_var(self.work, self.core, arm.span)?;
                        let opt = self.wpool.alloc_node(
                            self.work,
                            self.core,
                            WorkTypeNode::Option(elem),
                            arm.span,
                        )?;
                        self.wpool.unify(
                            scrut_ty,
                            opt,
                            self.env,
                            self.work,
                            self.core,
                            arm.span,
                            "expr.match",
                        )?;
                        if seen_opt[1] {
                            return Err(self.type_err(
                                "expr.match",
                                arm.span,
                                "unreachable match arm",
                            ));
                        }
                        seen_opt[1] = true;
                        if seen_opt[0] && seen_opt[1] {
                            caught_all = true;
                        }
                        if let Some(bname) = binding
                            && self
                                .scopes
                                .push_work(
                                    self.work,
                                    self.core,
                                    ScopeBinding {
                                        name: *bname,
                                        ty: elem,
                                    },
                                )
                                .is_none()
                        {
                            return Err(oom(self.core, arm.span));
                        }
                    }
                    Pattern::Ok(binding) => {
                        let tok = self.wpool.fresh_var(self.work, self.core, arm.span)?;
                        let terr = self.wpool.fresh_var(self.work, self.core, arm.span)?;
                        let res_ty = self.wpool.alloc_node(
                            self.work,
                            self.core,
                            WorkTypeNode::Result { ok: tok, err: terr },
                            arm.span,
                        )?;
                        self.wpool.unify(
                            scrut_ty,
                            res_ty,
                            self.env,
                            self.work,
                            self.core,
                            arm.span,
                            "expr.match",
                        )?;
                        if seen_res[0] {
                            return Err(self.type_err(
                                "expr.match",
                                arm.span,
                                "unreachable match arm",
                            ));
                        }
                        seen_res[0] = true;
                        if seen_res[0] && seen_res[1] {
                            caught_all = true;
                        }
                        if let Some(bname) = binding
                            && self
                                .scopes
                                .push_work(
                                    self.work,
                                    self.core,
                                    ScopeBinding {
                                        name: *bname,
                                        ty: tok,
                                    },
                                )
                                .is_none()
                        {
                            return Err(oom(self.core, arm.span));
                        }
                    }
                    Pattern::Err(binding) => {
                        let tok = self.wpool.fresh_var(self.work, self.core, arm.span)?;
                        let terr = self.wpool.fresh_var(self.work, self.core, arm.span)?;
                        let res_ty = self.wpool.alloc_node(
                            self.work,
                            self.core,
                            WorkTypeNode::Result { ok: tok, err: terr },
                            arm.span,
                        )?;
                        self.wpool.unify(
                            scrut_ty,
                            res_ty,
                            self.env,
                            self.work,
                            self.core,
                            arm.span,
                            "expr.match",
                        )?;
                        if seen_res[1] {
                            return Err(self.type_err(
                                "expr.match",
                                arm.span,
                                "unreachable match arm",
                            ));
                        }
                        seen_res[1] = true;
                        if seen_res[0] && seen_res[1] {
                            caught_all = true;
                        }
                        if let Some(bname) = binding
                            && self
                                .scopes
                                .push_work(
                                    self.work,
                                    self.core,
                                    ScopeBinding {
                                        name: *bname,
                                        ty: terr,
                                    },
                                )
                                .is_none()
                        {
                            return Err(oom(self.core, arm.span));
                        }
                    }
                    Pattern::DatumType { kind, binding } => {
                        self.wpool.unify(
                            scrut_ty,
                            WTY_DATUM,
                            self.env,
                            self.work,
                            self.core,
                            arm.span,
                            "expr.match",
                        )?;
                        let (idx, bind_ty) = match kind {
                            DatumTypePat::I64 => (0, WTY_I64),
                            DatumTypePat::F64 => (1, WTY_F64),
                            DatumTypePat::Bool => (2, WTY_BOOL),
                            DatumTypePat::Char => (3, WTY_CHAR),
                            DatumTypePat::Text => (4, WTY_TEXT),
                            DatumTypePat::Ident => (5, WTY_IDENT),
                            DatumTypePat::List => {
                                let ld = self.wpool.alloc_node(
                                    self.work,
                                    self.core,
                                    WorkTypeNode::List(WTY_DATUM),
                                    arm.span,
                                )?;
                                (6, ld)
                            }
                        };
                        if seen_datum[idx] {
                            return Err(self.type_err(
                                "expr.match",
                                arm.span,
                                "unreachable match arm",
                            ));
                        }
                        seen_datum[idx] = true;
                        if seen_datum.iter().all(|&x| x) {
                            caught_all = true;
                        }
                        if let Some(bname) = binding
                            && self
                                .scopes
                                .push_work(
                                    self.work,
                                    self.core,
                                    ScopeBinding {
                                        name: *bname,
                                        ty: bind_ty,
                                    },
                                )
                                .is_none()
                        {
                            return Err(oom(self.core, arm.span));
                        }
                    }
                    Pattern::Variant {
                        module,
                        type_name,
                        variant,
                        binding,
                    } => {
                        let sid = self.lookup_schema_ref(*module, *type_name, arm.span)?;
                        let var_ty = self.wpool.alloc_node(
                            self.work,
                            self.core,
                            WorkTypeNode::Schema(sid),
                            arm.span,
                        )?;
                        self.wpool.unify(
                            scrut_ty,
                            var_ty,
                            self.env,
                            self.work,
                            self.core,
                            arm.span,
                            "expr.match",
                        )?;
                        if seen_variant.is_empty()
                            && let Some(cnt) = self.variant_case_count(sid)
                        {
                            for _ in 0..cnt {
                                if seen_variant
                                    .push_work(self.work, self.core, false)
                                    .is_none()
                                {
                                    return Err(oom(self.core, arm.span));
                                }
                            }
                        }
                        let (case_idx, payload_ty) =
                            self.lookup_variant_case(sid, variant.as_str(), arm.span)?;
                        if seen_variant.as_slice()[case_idx] {
                            return Err(self.type_err(
                                "expr.match",
                                arm.span,
                                "unreachable match arm",
                            ));
                        }
                        seen_variant.as_mut_slice()[case_idx] = true;
                        if seen_variant.as_slice().iter().all(|&x| x) {
                            caught_all = true;
                        }
                        match (payload_ty, binding) {
                            (Some(pty), Some(bname)) => {
                                if self
                                    .scopes
                                    .push_work(
                                        self.work,
                                        self.core,
                                        ScopeBinding {
                                            name: *bname,
                                            ty: pty,
                                        },
                                    )
                                    .is_none()
                                {
                                    return Err(oom(self.core, arm.span));
                                }
                            }
                            (None, Some(_)) => {
                                return Err(self.type_err(
                                    "expr.match",
                                    arm.span,
                                    "variant case has no payload to bind",
                                ));
                            }
                            (_, None) => {}
                        }
                    }
                }

                let body = arm.body.as_slice();
                for e in &body[..body.len().saturating_sub(1)] {
                    let _ = self.check_expr(e, None)?;
                }
                let arm_ty = match body.last() {
                    Some(last) => self.check_expr(last, result_ty)?,
                    None => WTY_VOID,
                };
                match result_ty {
                    Some(rt) => {
                        let _ = self.wpool.coerce_to_expected(
                            arm_ty,
                            rt,
                            self.env,
                            self.work,
                            self.core,
                            arm.span,
                            "expr.match",
                        )?;
                        Ok(rt)
                    }
                    None => {
                        result_ty = Some(arm_ty);
                        Ok(arm_ty)
                    }
                }
            })();
            self.scopes.truncate(saved);
            arm_res?;
        }

        if !caught_all {
            return Err(self.type_err("expr.match", span, "non-exhaustive match"));
        }
        Ok(result_ty.unwrap_or(WTY_VOID))
    }

    fn commit_to_perm(&mut self) -> Result<(), ErrRecorded> {
        let mod_name_perm = match self.opts.module_name {
            Some(mn) => match RawStr::from_perm(self.perm, self.core, mn) {
                Some(rs) => Some(rs),
                None => return Err(oom(self.core, Span::default())),
            },
            None => None,
        };

        // 1. ローカルスキーマを 2 パスで永続領域へ確定
        self.wpool
            .commit_local_schemas(mod_name_perm, self.env, self.perm, self.core)?;

        if let Some(mname) = mod_name_perm {
            let mut exports_types = RawMap::new();
            let mut exports_vals = RawMap::new();
            let should_export = |name: &str, has_exp: bool, list: &[(RawStr, Span)]| -> bool {
                !has_exp || list.iter().any(|(n, _)| n.as_str() == name)
            };

            for i in 0..self.wpool.local_schemas.len() {
                let ls = self.wpool.local_schemas.as_slice()[i];
                if should_export(
                    ls.name.as_str(),
                    self.has_export,
                    self.explicit_exports.as_slice(),
                ) {
                    let sid = ls.perm_id.expect("committed");
                    let name_p = self.env.get_schema(sid).name;
                    let h = fnv1a_bytes(name_p.as_bytes());
                    if exports_types
                        .insert_perm(self.perm, self.core, h, name_p, sid, |a, b| {
                            a.as_str() == b.as_str()
                        })
                        .is_none()
                    {
                        return Err(oom(self.core, ls.span));
                    }
                }
            }

            for i in 0..self.local_defs.len() {
                let info = self.local_defs.as_slice()[i];
                if should_export(
                    info.name.as_str(),
                    self.has_export,
                    self.explicit_exports.as_slice(),
                ) {
                    let tid = self
                        .wpool
                        .commit_type(info.ty, self.env, self.perm, self.core, info.span)?;
                    let Some(name_p) = RawStr::from_perm(self.perm, self.core, info.name.as_str())
                    else {
                        return Err(oom(self.core, info.span));
                    };
                    let h = fnv1a_bytes(name_p.as_bytes());
                    if exports_vals
                        .insert_perm(
                            self.perm,
                            self.core,
                            h,
                            name_p,
                            ValBinding {
                                ty: tid,
                                fn_sig: None,
                            },
                            |a, b| a.as_str() == b.as_str(),
                        )
                        .is_none()
                    {
                        return Err(oom(self.core, info.span));
                    }
                }
            }

            for i in 0..self.local_defns.len() {
                let info = self.local_defns.as_slice()[i];
                if should_export(
                    info.name.as_str(),
                    self.has_export,
                    self.explicit_exports.as_slice(),
                ) {
                    let vb = self.commit_defn_binding(info)?;
                    let Some(name_p) = RawStr::from_perm(self.perm, self.core, info.name.as_str())
                    else {
                        return Err(oom(self.core, info.span));
                    };
                    let h = fnv1a_bytes(name_p.as_bytes());
                    if exports_vals
                        .insert_perm(self.perm, self.core, h, name_p, vb, |a, b| {
                            a.as_str() == b.as_str()
                        })
                        .is_none()
                    {
                        return Err(oom(self.core, info.span));
                    }
                }
            }

            let _ = self.env.define_module(
                self.perm,
                self.core,
                ModuleTypeEnv {
                    name: mname,
                    exports_types,
                    exports_vals,
                },
                Span::default(),
            )?;
        } else {
            for i in 0..self.wpool.local_schemas.len() {
                let ls = self.wpool.local_schemas.as_slice()[i];
                let sid = ls.perm_id.expect("committed");
                self.env
                    .define_root_type(self.perm, self.core, ls.name.as_str(), sid, ls.span)?;
            }
            for i in 0..self.local_defs.len() {
                let info = self.local_defs.as_slice()[i];
                let tid = self
                    .wpool
                    .commit_type(info.ty, self.env, self.perm, self.core, info.span)?;
                self.env.define_root_val(
                    self.perm,
                    self.core,
                    info.name.as_str(),
                    ValBinding {
                        ty: tid,
                        fn_sig: None,
                    },
                    info.span,
                )?;
            }
            for i in 0..self.local_defns.len() {
                let info = self.local_defns.as_slice()[i];
                let vb = self.commit_defn_binding(info)?;
                self.env.define_root_val(
                    self.perm,
                    self.core,
                    info.name.as_str(),
                    vb,
                    info.span,
                )?;
            }
        }
        Ok(())
    }

    fn commit_defn_binding(&mut self, info: LocalDefnInfo) -> Result<ValBinding, ErrRecorded> {
        let fn_tid = self
            .wpool
            .commit_type(info.fn_ty, self.env, self.perm, self.core, info.span)?;
        let ret_tid = self
            .wpool
            .commit_type(info.ret, self.env, self.perm, self.core, info.span)?;
        let mut perm_params = RawVec::new();
        for &(pname, pty) in info.params.as_slice() {
            let Some(pn_perm) = RawStr::from_perm(self.perm, self.core, pname.as_str()) else {
                return Err(oom(self.core, info.span));
            };
            let ptid = self
                .wpool
                .commit_type(pty, self.env, self.perm, self.core, info.span)?;
            if perm_params
                .push_perm(self.perm, self.core, (pn_perm, ptid))
                .is_none()
            {
                return Err(oom(self.core, info.span));
            }
        }
        Ok(ValBinding {
            ty: fn_tid,
            fn_sig: Some(FnSig {
                params: perm_params,
                ret: ret_tid,
            }),
        })
    }
}

fn lookup_builtin_by_name(name: &str) -> Option<BuiltinId> {
    BuiltinId::ALL.iter().find(|&&b| b.name() == name).copied()
}

/// 入力 AST 節数と型式節数の合計 `N` を数える（非再帰）。
fn count_program_nodes(
    exprs: &[Expr],
    work: &mut WorkArena,
    core: &mut AllocCore<'_>,
    span: Span,
) -> Result<usize, ErrRecorded> {
    let mut count = 0usize;
    let mut stack = RawVec::<&Expr>::new();
    for e in exprs {
        if stack.push_work(work, core, e).is_none() {
            return Err(oom(core, span));
        }
    }
    while let Some(e) = stack.pop() {
        count = count.saturating_add(1);
        match &e.kind {
            ExprKind::List(xs) | ExprKind::Tuple(xs) | ExprKind::Template(xs) => {
                for x in xs.as_slice() {
                    if stack.push_work(work, core, x).is_none() {
                        return Err(oom(core, x.span));
                    }
                }
            }
            ExprKind::Struct(s) => {
                for (_, v) in s.fields.as_slice() {
                    if stack.push_work(work, core, v).is_none() {
                        return Err(oom(core, v.span));
                    }
                }
            }
            ExprKind::Variant(v) => {
                if let Some(d) = &v.data
                    && stack.push_work(work, core, d.as_ref()).is_none()
                {
                    return Err(oom(core, d.span));
                }
            }
            ExprKind::Some(x)
            | ExprKind::Ok(x)
            | ExprKind::Err(x)
            | ExprKind::Field { target: x, .. } => {
                if stack.push_work(work, core, x.as_ref()).is_none() {
                    return Err(oom(core, x.span));
                }
            }
            ExprKind::Def(d) => {
                if let Some(tb) = d.ty {
                    count = count.saturating_add(count_type_expr_nodes(tb.as_ref(), work, core)?);
                }
                if stack.push_work(work, core, d.value.as_ref()).is_none() {
                    return Err(oom(core, d.value.span));
                }
            }
            ExprKind::DefSchema(ds) => match &ds.body {
                SchemaBody::Struct { fields, .. } => {
                    for (_, fty) in fields.as_slice() {
                        count = count.saturating_add(1);
                        count = count.saturating_add(count_type_expr_nodes(fty, work, core)?);
                    }
                }
                SchemaBody::Variant { cases, .. } => {
                    for c in cases.as_slice() {
                        count = count.saturating_add(1);
                        if let Some(pb) = c.payload {
                            count = count.saturating_add(count_type_expr_nodes(
                                pb.as_ref(),
                                work,
                                core,
                            )?);
                        }
                    }
                }
                SchemaBody::CloneOf { .. } => {
                    count = count.saturating_add(1);
                }
            },
            ExprKind::Defn(d) => {
                for p in d.params.as_slice() {
                    if let Some(tb) = p.ty {
                        count =
                            count.saturating_add(count_type_expr_nodes(tb.as_ref(), work, core)?);
                    }
                }
                if let Some(rb) = d.ret_ty {
                    count = count.saturating_add(count_type_expr_nodes(rb.as_ref(), work, core)?);
                }
                for x in d.body.as_slice() {
                    if stack.push_work(work, core, x).is_none() {
                        return Err(oom(core, x.span));
                    }
                }
            }
            ExprKind::Fn(f) => {
                for p in f.params.as_slice() {
                    if let Some(tb) = p.ty {
                        count =
                            count.saturating_add(count_type_expr_nodes(tb.as_ref(), work, core)?);
                    }
                }
                if let Some(rb) = f.ret_ty {
                    count = count.saturating_add(count_type_expr_nodes(rb.as_ref(), work, core)?);
                }
                for x in f.body.as_slice() {
                    if stack.push_work(work, core, x).is_none() {
                        return Err(oom(core, x.span));
                    }
                }
            }
            ExprKind::Let(l) => {
                for (_, v) in l.bindings.as_slice() {
                    if stack.push_work(work, core, v).is_none() {
                        return Err(oom(core, v.span));
                    }
                }
                for x in l.body.as_slice() {
                    if stack.push_work(work, core, x).is_none() {
                        return Err(oom(core, x.span));
                    }
                }
            }
            ExprKind::Index { target, index } => {
                if stack.push_work(work, core, target.as_ref()).is_none()
                    || stack.push_work(work, core, index.as_ref()).is_none()
                {
                    return Err(oom(core, e.span));
                }
            }
            ExprKind::With { target, fields } => {
                if stack.push_work(work, core, target.as_ref()).is_none() {
                    return Err(oom(core, target.span));
                }
                for (_, v) in fields.as_slice() {
                    if stack.push_work(work, core, v).is_none() {
                        return Err(oom(core, v.span));
                    }
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                if stack.push_work(work, core, cond.as_ref()).is_none()
                    || stack.push_work(work, core, then_branch.as_ref()).is_none()
                    || stack.push_work(work, core, else_branch.as_ref()).is_none()
                {
                    return Err(oom(core, e.span));
                }
            }
            ExprKind::When { cond, body } => {
                if stack.push_work(work, core, cond.as_ref()).is_none() {
                    return Err(oom(core, cond.span));
                }
                for x in body.as_slice() {
                    if stack.push_work(work, core, x).is_none() {
                        return Err(oom(core, x.span));
                    }
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                if stack.push_work(work, core, scrutinee.as_ref()).is_none() {
                    return Err(oom(core, scrutinee.span));
                }
                for arm in arms.as_slice() {
                    count = count.saturating_add(1);
                    for x in arm.body.as_slice() {
                        if stack.push_work(work, core, x).is_none() {
                            return Err(oom(core, x.span));
                        }
                    }
                }
            }
            ExprKind::Call { callee, args } => {
                if stack.push_work(work, core, callee.as_ref()).is_none() {
                    return Err(oom(core, callee.span));
                }
                for x in args.as_slice() {
                    if stack.push_work(work, core, x).is_none() {
                        return Err(oom(core, x.span));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(count)
}

fn count_type_expr_nodes(
    root: &TypeExpr,
    work: &mut WorkArena,
    core: &mut AllocCore<'_>,
) -> Result<usize, ErrRecorded> {
    let mut count = 0usize;
    let mut stack = RawVec::<&TypeExpr>::new();
    if stack.push_work(work, core, root).is_none() {
        return Err(oom(core, root.span));
    }
    while let Some(t) = stack.pop() {
        count = count.saturating_add(1);
        match &t.kind {
            TypeExprKind::List(x) | TypeExprKind::Array(x) | TypeExprKind::Option(x) => {
                if stack.push_work(work, core, x.as_ref()).is_none() {
                    return Err(oom(core, x.span));
                }
            }
            TypeExprKind::Result { ok, err } => {
                if stack.push_work(work, core, ok.as_ref()).is_none()
                    || stack.push_work(work, core, err.as_ref()).is_none()
                {
                    return Err(oom(core, t.span));
                }
            }
            TypeExprKind::Tuple(xs) => {
                for x in xs.as_slice() {
                    if stack.push_work(work, core, x).is_none() {
                        return Err(oom(core, x.span));
                    }
                }
            }
            TypeExprKind::Fn { params, ret } => {
                for p in params.as_slice() {
                    if stack.push_work(work, core, p).is_none() {
                        return Err(oom(core, p.span));
                    }
                }
                if stack.push_work(work, core, ret.as_ref()).is_none() {
                    return Err(oom(core, ret.span));
                }
            }
            _ => {}
        }
    }
    Ok(count)
}
