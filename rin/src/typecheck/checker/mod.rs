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

mod commit;
mod decls;
mod expr;
mod expr_forms;
mod match_expr;

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
    /// 一時的な初期束縛（`eval_with` の永続型ハンドル用）。
    pub(crate) extra_bindings: &'a [(&'a str, TypeId)],
    /// 一時的な初期束縛（`eval_with` の型式 AST 用・凍結環境で `PermArena` 不使用）。
    pub(crate) extra_binding_exprs: &'a [(&'a str, TypeExpr)],
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
            extra_binding_exprs: &[],
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
    pub(in crate::typecheck) env: &'a mut TypeEnv,
    pub(in crate::typecheck) perm: &'a mut PermArena,
    pub(in crate::typecheck) work: &'a mut WorkArena,
    pub(in crate::typecheck) core: &'a mut AllocCore<'alloc>,
    pub(in crate::typecheck) wpool: WorkTypePool,
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
    local_imports: RawVec<(RawStr, u32, Span)>,
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
    let node_count = commit::count_program_nodes(exprs, work, core, first_span)?;
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
        local_imports: RawVec::new(),
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
    let nodes = commit::count_type_expr_nodes(ty_expr, work, core)?.max(1);
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
            frozen: true,
            strict_defn_annotations: true,
            module_name: None,
            test_mode: false,
            commit: false,
            extra_bindings: &[],
            extra_binding_exprs: &[],
        },
        local_type_map: RawMap::new(),
        local_val_map: RawMap::new(),
        local_defs: RawVec::new(),
        local_defns: RawVec::new(),
        imported_modules: RawMap::new(),
        local_imports: RawVec::new(),
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
        //    `scopes` は `run` 内部でのみ参照され永続化されないため、`'a` の `&str` を複製せず借用する。
        for &(name, tid) in self.opts.extra_bindings {
            // SAFETY: `name` は `CheckOptions<'a>` の寿命 `'a` の間有効で、`scopes` は `run` を超えて生存しない。
            let rs = unsafe { RawStr::from_borrowed(name) };
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
        for (name, te) in self.opts.extra_binding_exprs {
            // SAFETY: `name` は `CheckOptions<'a>` の寿命 `'a` の間有効で、`scopes` は `run` を超えて生存しない。
            let rs = unsafe { RawStr::from_borrowed(name) };
            let wty = self.resolve_type_expr(te)?;
            if self
                .scopes
                .push_work(self.work, self.core, ScopeBinding { name: rs, ty: wty })
                .is_none()
            {
                return Err(oom(self.core, te.span));
            }
        }

        // 1. import / export の凍結検査と import の収集
        for expr in exprs {
            match expr.kind {
                ExprKind::Import { name } => {
                    if self.opts.frozen {
                        return Err(self.core.err_str(
                            "env.frozen",
                            ErrorKind::Parse,
                            expr.span,
                            "cannot use import in frozen environment",
                        ));
                    }
                    let Some((slot_idx, _)) = self.env.lookup_module(name.as_str()) else {
                        use std::fmt::Write as _;
                        let out =
                            self.core
                                .error
                                .begin_msg("expr.module", ErrorKind::Runtime, expr.span);
                        let _ = write!(out, "unknown module '{}'", name.as_str());
                        crate::diagnostic::write_suggest(
                            out,
                            name.as_str(),
                            self.env.iter_module_names(),
                        );
                        return Err(ErrRecorded);
                    };
                    let h = fnv1a_bytes(name.as_bytes());
                    if self
                        .imported_modules
                        .insert_work(self.work, self.core, h, name, slot_idx, |a, b| {
                            a.as_str() == b.as_str()
                        })
                        .is_none()
                        || self
                            .local_imports
                            .push_work(self.work, self.core, (name, slot_idx, expr.span))
                            .is_none()
                    {
                        return Err(oom(self.core, expr.span));
                    }
                }
                ExprKind::Export { .. } if self.opts.frozen => {
                    return Err(self.core.err_str(
                        "env.frozen",
                        ErrorKind::Parse,
                        expr.span,
                        "cannot use export in frozen environment",
                    ));
                }
                _ => {}
            }
        }

        // 2. defschema 名の事前登録（前方参照・相互再帰を許可）
        for expr in exprs {
            if let ExprKind::DefSchema(ds) = expr.kind {
                if self.opts.frozen {
                    return Err(self.core.err_str(
                        "env.frozen",
                        ErrorKind::Parse,
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
                        ErrorKind::Parse,
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
                            ErrorKind::Parse,
                            expr.span,
                            "cannot use def in frozen environment",
                        ));
                    }
                    let d = d.as_ref();
                    self.check_no_val_redef(d.name.as_str(), expr.span, "expr.def")?;
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
}

#[inline]
fn lookup_builtin_by_name(name: &str) -> Option<BuiltinId> {
    let id = crate::bytecode::static_sym_id(name)?;
    BuiltinId::ALL.get(id as usize).copied()
}
