//! 宣言（`defschema`・`defn`）の事前登録・解決および型式解決。

use super::*;

impl<'a> TypeChecker<'a, '_> {
    pub(super) fn preregister_defschema(
        &mut self,
        ds: &DefSchemaExpr,
        span: Span,
    ) -> Result<(), ErrRecorded> {
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

    pub(super) fn resolve_all_defschemas(
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
                    wfields
                        .as_mut_slice()
                        .sort_unstable_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
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

    pub(super) fn clone_perm_schema_to_local(
        &mut self,
        pid: crate::typecheck::types::SchemaId,
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

    pub(super) fn preregister_defn(
        &mut self,
        d: &crate::ast::DefnExpr,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        self.check_no_val_redef(d.name.as_str(), span, "expr.defn")?;
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

    pub(super) fn check_defn_body(
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

    pub(super) fn check_no_val_redef(
        &mut self,
        name: &str,
        span: Span,
        rule: &'static str,
    ) -> Result<(), ErrRecorded> {
        let emit = |core: &mut AllocCore<'_>, msg: &str| -> ErrRecorded {
            if rule == "expr.defn" {
                core.err_str("expr.defn", ErrorKind::Runtime, span, msg)
            } else {
                core.err_str("expr.def", ErrorKind::Runtime, span, msg)
            }
        };
        let h = fnv1a_bytes(name.as_bytes());
        if self
            .local_val_map
            .get_by(h, |k| k.as_str() == name)
            .is_some()
        {
            return Err(emit(self.core, "cannot redefine name in same unit"));
        }
        if self.opts.module_name.is_none() && self.env.lookup_root_val(name).is_some() {
            return Err(emit(self.core, "cannot redefine existing global name"));
        }
        if crate::bytecode::static_sym_id(name).is_some() {
            return Err(if rule == "expr.defn" {
                self.core.err_fmt(
                    "expr.defn",
                    ErrorKind::Runtime,
                    span,
                    format_args!("cannot redefine builtin '{name}'"),
                )
            } else {
                self.core.err_fmt(
                    "expr.def",
                    ErrorKind::Runtime,
                    span,
                    format_args!("cannot redefine builtin '{name}'"),
                )
            });
        }
        if self.opts.module_name.is_none() && self.env.lookup_module(name).is_some() {
            return Err(emit(self.core, "cannot redefine loaded module name"));
        }
        Ok(())
    }

    pub(super) fn lookup_schema_ref(
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
                    if self.opts.module_name.is_none() {
                        self.env.lookup_root_import(mname.as_str())
                    } else {
                        None
                    }
                })
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

    pub(super) fn add_perm_schema_budget(&mut self, sid: crate::typecheck::types::SchemaId) {
        let items = match self.env.get_schema(sid).kind {
            SchemaKind::Struct { fields, .. } => fields.len(),
            SchemaKind::Variant { cases, .. } => cases.len(),
        };
        self.wpool.budget_remaining = self
            .wpool
            .budget_remaining
            .saturating_add(items.saturating_mul(TYPECHECK_WORK_MULTIPLIER));
    }

    pub(in crate::typecheck) fn resolve_type_expr(
        &mut self,
        te: &TypeExpr,
    ) -> Result<WorkTypeId, ErrRecorded> {
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
}
