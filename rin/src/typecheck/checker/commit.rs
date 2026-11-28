//! 型検査結果の `PermArena` への確定と AST 節数カウント。

use super::*;

impl<'a> TypeChecker<'a, '_> {
    pub(super) fn commit_to_perm(&mut self) -> Result<(), ErrRecorded> {
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
            let mut export_val_names = RawVec::<RawStr>::new();
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
                    if export_val_names
                        .push_perm(self.perm, self.core, name_p)
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
                    if export_val_names
                        .push_perm(self.perm, self.core, name_p)
                        .is_none()
                    {
                        return Err(oom(self.core, info.span));
                    }
                }
            }
            export_val_names
                .as_mut_slice()
                .sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));

            let _ = self.env.define_module(
                self.perm,
                self.core,
                ModuleTypeEnv {
                    name: mname,
                    exports_types,
                    exports_vals,
                    export_val_names,
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
            for i in 0..self.local_imports.len() {
                let (iname, islot, ispan) = self.local_imports.as_slice()[i];
                self.env
                    .define_root_import(self.perm, self.core, iname.as_str(), islot, ispan)?;
            }
        }
        Ok(())
    }

    pub(super) fn commit_defn_binding(
        &mut self,
        info: LocalDefnInfo,
    ) -> Result<ValBinding, ErrRecorded> {
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

/// 入力 AST 節数と型式節数の合計 `N` を数える（非再帰）。
pub(super) fn count_program_nodes(
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

pub(super) fn count_type_expr_nodes(
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
