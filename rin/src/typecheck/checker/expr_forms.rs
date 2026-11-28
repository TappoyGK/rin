//! 複合式（`let`・`fn`・リスト・準クォート・構造体・バリアント・名前解決）の型検査。

use super::*;

impl<'a> TypeChecker<'a, '_> {
    #[inline(never)]
    pub(super) fn check_let_chain(
        &mut self,
        first: &crate::ast::LetExpr,
        expected: Option<WorkTypeId>,
    ) -> Result<WorkTypeId, ErrRecorded> {
        let saved = self.scopes.len();
        let res = (|| -> Result<WorkTypeId, ErrRecorded> {
            let mut cur = first;
            loop {
                for &(bname, ref bexpr) in cur.bindings.as_slice() {
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
                let body = cur.body.as_slice();
                for e in &body[..body.len().saturating_sub(1)] {
                    let _ = self.check_expr(e, None)?;
                }
                match body.last() {
                    Some(last) => {
                        if let ExprKind::Let(next_l) = &last.kind {
                            self.wpool.tick(self.core, last.span)?;
                            cur = next_l.as_ref();
                            continue;
                        }
                        return self.check_expr(last, expected);
                    }
                    None => return Ok(WTY_VOID),
                }
            }
        })();
        self.scopes.truncate(saved);
        res
    }

    #[inline(never)]
    pub(super) fn check_fn_chain(
        &mut self,
        first: &crate::ast::FnExpr,
        expected: Option<WorkTypeId>,
        first_span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        let saved = self.scopes.len();
        let res = (|| -> Result<WorkTypeId, ErrRecorded> {
            let mut frames = RawVec::<(RawVec<WorkTypeId>, Option<WorkTypeId>, Span)>::new();
            let mut cur = first;
            let mut cur_exp = expected;
            let mut cur_span = first_span;
            let mut inner_ty = loop {
                let (exp_params, exp_ret) = match cur_exp {
                    Some(e) => match self
                        .wpool
                        .unwrap_perm(e, self.env, self.work, self.core, cur_span)?
                    {
                        WorkTypeNode::Fn { params, ret } if params.len() == cur.params.len() => {
                            (Some(params), Some(ret))
                        }
                        _ => (None, None),
                    },
                    None => (None, None),
                };
                let mut p_tys = RawVec::new();
                for (i, p) in cur.params.as_slice().iter().enumerate() {
                    let exp_pi = exp_params.map(|ps| ps.as_slice()[i]);
                    let pty = match (p.ty, exp_pi) {
                        (Some(tb), Some(ep)) => {
                            let t = self.resolve_type_expr(tb.as_ref())?;
                            self.wpool
                                .unify(t, ep, self.env, self.work, self.core, tb.span, "expr.fn")?;
                            t
                        }
                        (Some(tb), None) => self.resolve_type_expr(tb.as_ref())?,
                        (None, Some(ep)) => ep,
                        (None, None) => self.wpool.fresh_var(self.work, self.core, cur_span)?,
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
                        return Err(oom(self.core, cur_span));
                    }
                }
                let ret_exp = match (cur.ret_ty, exp_ret) {
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
                if frames
                    .push_work(self.work, self.core, (p_tys, ret_exp, cur_span))
                    .is_none()
                {
                    return Err(oom(self.core, cur_span));
                }
                let body = cur.body.as_slice();
                for e in &body[..body.len().saturating_sub(1)] {
                    let _ = self.check_expr(e, None)?;
                }
                match body.last() {
                    Some(last) => {
                        if let ExprKind::Fn(next_f) = &last.kind {
                            self.wpool.tick(self.core, last.span)?;
                            cur = next_f.as_ref();
                            cur_exp = ret_exp;
                            cur_span = last.span;
                            continue;
                        }
                        break self.check_expr(last, ret_exp)?;
                    }
                    None => break WTY_VOID,
                }
            };
            while let Some((p_tys, ret_exp, fspan)) = frames.pop() {
                let final_ret = match ret_exp {
                    Some(re) => self.wpool.coerce_to_expected(
                        inner_ty, re, self.env, self.work, self.core, fspan, "expr.fn",
                    )?,
                    None => inner_ty,
                };
                inner_ty = self.wpool.alloc_node(
                    self.work,
                    self.core,
                    WorkTypeNode::Fn {
                        params: p_tys,
                        ret: final_ret,
                    },
                    fspan,
                )?;
            }
            Ok(inner_ty)
        })();
        self.scopes.truncate(saved);
        res
    }

    #[inline(never)]
    pub(super) fn check_list_chain(
        &mut self,
        first_items: &[Expr],
        expected: Option<WorkTypeId>,
        first_span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        // 単一要素の入れ子リスト `[[[...x...]]]` を反復処理して小スタック消費を抑える。
        if first_items.len() != 1 || !matches!(first_items[0].kind, ExprKind::List(_)) {
            return self.check_list_elements(first_items, expected, first_span);
        }
        let mut wraps = RawVec::<(Option<WorkTypeId>, Span)>::new();
        let mut cur_items = first_items;
        let mut cur_exp = expected;
        let mut cur_span = first_span;
        let mut inner_ty = loop {
            if cur_items.len() == 1
                && let ExprKind::List(next_items) = &cur_items[0].kind
            {
                self.wpool.tick(self.core, cur_items[0].span)?;
                let elem_exp = self.expected_list_elem(cur_exp, cur_span)?;
                if wraps
                    .push_work(self.work, self.core, (elem_exp, cur_span))
                    .is_none()
                {
                    return Err(oom(self.core, cur_span));
                }
                cur_span = cur_items[0].span;
                cur_items = next_items.as_slice();
                cur_exp = elem_exp;
                continue;
            }
            break self.check_list_elements(cur_items, cur_exp, cur_span)?;
        };
        while let Some((elem_exp, lspan)) = wraps.pop() {
            let elem = match elem_exp {
                Some(ee) => self.wpool.coerce_to_expected(
                    inner_ty,
                    ee,
                    self.env,
                    self.work,
                    self.core,
                    lspan,
                    "expr.list",
                )?,
                None => inner_ty,
            };
            inner_ty =
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(elem), lspan)?;
        }
        Ok(inner_ty)
    }

    pub(in crate::typecheck) fn check_list_elements(
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

    pub(super) fn check_quasiquote_datum(&mut self, root: &Datum) -> Result<(), ErrRecorded> {
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

    pub(super) fn lookup_value_name(
        &mut self,
        name: &str,
        span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
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
        if self.opts.module_name.is_none()
            && let Some(slot_idx) = self.env.lookup_root_import(name)
        {
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
        use std::fmt::Write as _;
        let out = self
            .core
            .error
            .begin_msg("expr.name", ErrorKind::Type, span);
        let _ = write!(out, "unknown name '{name}'");
        let scopes_iter = self.scopes.as_slice().iter().map(|b| b.name.as_str());
        let defs_iter = self.local_defs.as_slice().iter().map(|d| d.name.as_str());
        let defns_iter = self.local_defns.as_slice().iter().map(|d| d.name.as_str());
        let imports_iter = self
            .local_imports
            .as_slice()
            .iter()
            .map(|(n, _, _)| n.as_str());
        let builtins_iter = BuiltinId::ALL
            .iter()
            .map(|b| b.name())
            .chain(std::iter::once("pi"));
        if self.opts.module_name.is_none() {
            let root_vals = self.env.iter_root_val_names();
            if self.opts.frozen {
                let mods = self.env.iter_module_names();
                crate::diagnostic::write_suggest(
                    out,
                    name,
                    scopes_iter
                        .chain(defs_iter)
                        .chain(defns_iter)
                        .chain(imports_iter)
                        .chain(root_vals)
                        .chain(builtins_iter)
                        .chain(mods),
                );
            } else {
                crate::diagnostic::write_suggest(
                    out,
                    name,
                    scopes_iter
                        .chain(defs_iter)
                        .chain(defns_iter)
                        .chain(imports_iter)
                        .chain(root_vals)
                        .chain(builtins_iter),
                );
            }
        } else {
            crate::diagnostic::write_suggest(
                out,
                name,
                scopes_iter
                    .chain(defs_iter)
                    .chain(defns_iter)
                    .chain(imports_iter)
                    .chain(builtins_iter),
            );
        }
        Err(ErrRecorded)
    }

    pub(super) fn lookup_struct_field_type(
        &mut self,
        sid: WorkSchemaId,
        field: &str,
        span: Span,
        rule: &'static str,
    ) -> Result<WorkTypeId, ErrRecorded> {
        use std::fmt::Write as _;
        match sid {
            WorkSchemaId::Perm(pid) => {
                let def = *self.env.get_schema(pid);
                let SchemaKind::Struct {
                    type_name, fields, ..
                } = def.kind
                else {
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
                let out = self.core.error.begin_msg(rule, ErrorKind::Type, span);
                let _ = write!(out, "{} has no field '{field}'", type_name.as_str());
                crate::diagnostic::write_available(
                    out,
                    "fields",
                    fields.as_slice().iter().map(|(f, _)| f.as_str()),
                );
                Err(ErrRecorded)
            }
            WorkSchemaId::Local(lidx) => {
                let kind = self.wpool.local_schemas.as_slice()[lidx as usize]
                    .kind
                    .expect("local schema resolved");
                let LocalSchemaKind::Struct {
                    type_name, fields, ..
                } = kind
                else {
                    return Err(self.type_err(rule, span, "target is not a struct"));
                };
                for &(fname, fty) in fields.as_slice() {
                    if fname.as_str() == field {
                        return Ok(fty);
                    }
                }
                let out = self.core.error.begin_msg(rule, ErrorKind::Type, span);
                let _ = write!(out, "{} has no field '{field}'", type_name.as_str());
                crate::diagnostic::write_available(
                    out,
                    "fields",
                    fields.as_slice().iter().map(|(f, _)| f.as_str()),
                );
                Err(ErrRecorded)
            }
        }
    }

    pub(super) fn check_struct_literal(
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

    pub(super) fn check_variant_literal(
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

    pub(super) fn lookup_variant_case(
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

    pub(super) fn variant_case_count(&self, sid: WorkSchemaId) -> Option<usize> {
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
}
