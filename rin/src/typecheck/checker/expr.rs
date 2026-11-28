//! 式ノードの双方向型検査（`check_expr` 本体）。

use super::*;

impl<'a> TypeChecker<'a, '_> {
    /// 双方向型検査の本体。`expected` があれば内側へ期待型を伝播する。
    pub(in crate::typecheck) fn check_expr(
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
            ExprKind::List(items) => self.check_list_chain(items.as_slice(), expected, span),
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
                let sid = self.lookup_schema_ref(s.module, s.name, span)?;
                self.check_struct_literal(sid, s.fields.as_slice(), span)
            }
            ExprKind::Variant(v) => {
                let v = v.as_ref();
                let sid = self.lookup_schema_ref(v.module, v.type_name, span)?;
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
                let mut chain = RawVec::<(RawStr, Span)>::new();
                if chain
                    .push_work(self.work, self.core, (*field, span))
                    .is_none()
                {
                    return Err(oom(self.core, span));
                }
                let mut cur = target.as_ref();
                while let ExprKind::Field {
                    target: t,
                    field: f,
                } = &cur.kind
                {
                    self.wpool.tick(self.core, cur.span)?;
                    if chain
                        .push_work(self.work, self.core, (*f, cur.span))
                        .is_none()
                    {
                        return Err(oom(self.core, cur.span));
                    }
                    cur = t.as_ref();
                }
                let mut cur_ty = self.check_expr(cur, None)?;
                let mut cur_span = cur.span;
                for &(f, fspan) in chain.as_slice().iter().rev() {
                    let node = self
                        .wpool
                        .unwrap_perm(cur_ty, self.env, self.work, self.core, cur_span)?;
                    cur_ty = match node {
                        WorkTypeNode::Module(slot_idx) => {
                            let Some(menv) = self.env.get_module(slot_idx) else {
                                return Err(self.type_err("expr.module", fspan, "unknown module"));
                            };
                            let h = fnv1a_bytes(f.as_bytes());
                            let Some(vb) =
                                menv.exports_vals.get_by(h, |k| k.as_str() == f.as_str())
                            else {
                                use std::fmt::Write as _;
                                let mname = menv.name;
                                if menv
                                    .exports_types
                                    .get_by(h, |k| k.as_str() == f.as_str())
                                    .is_some()
                                {
                                    return Err(self.core.err_fmt(
                                        "expr.module",
                                        ErrorKind::Type,
                                        fspan,
                                        format_args!(
                                            "'{}.{}' is a type, not a value",
                                            mname.as_str(),
                                            f.as_str()
                                        ),
                                    ));
                                }
                                let names = menv.export_val_names;
                                let out = self.core.error.begin_msg(
                                    "expr.module",
                                    ErrorKind::Type,
                                    fspan,
                                );
                                let _ = write!(
                                    out,
                                    "module '{}' has no export '{}'",
                                    mname.as_str(),
                                    f.as_str()
                                );
                                crate::diagnostic::write_available(
                                    out,
                                    "exports",
                                    names.as_slice().iter().map(|s| s.as_str()),
                                );
                                return Err(ErrRecorded);
                            };
                            self.wpool.alloc_node(
                                self.work,
                                self.core,
                                WorkTypeNode::Perm(vb.ty),
                                fspan,
                            )?
                        }
                        WorkTypeNode::Schema(sid) => {
                            self.lookup_struct_field_type(sid, f.as_str(), fspan, "expr.field")?
                        }
                        _ => {
                            return Err(self.type_err(
                                "expr.field",
                                fspan,
                                "field access requires a struct or module",
                            ));
                        }
                    };
                    cur_span = fspan;
                }
                Ok(cur_ty)
            }
            ExprKind::Index { target, index } => {
                let mut indices = RawVec::<(&Expr, Span)>::new();
                if indices
                    .push_work(self.work, self.core, (index.as_ref(), span))
                    .is_none()
                {
                    return Err(oom(self.core, span));
                }
                let mut cur = target.as_ref();
                while let ExprKind::Index {
                    target: t,
                    index: i,
                } = &cur.kind
                {
                    self.wpool.tick(self.core, cur.span)?;
                    if indices
                        .push_work(self.work, self.core, (i.as_ref(), cur.span))
                        .is_none()
                    {
                        return Err(oom(self.core, cur.span));
                    }
                    cur = t.as_ref();
                }
                let mut cur_ty = self.check_expr(cur, None)?;
                let mut cur_span = cur.span;
                for &(idx_expr, ispan) in indices.as_slice().iter().rev() {
                    let ti = self.check_expr(idx_expr, Some(WTY_I64))?;
                    self.wpool.unify(
                        ti,
                        WTY_I64,
                        self.env,
                        self.work,
                        self.core,
                        idx_expr.span,
                        "expr.index",
                    )?;
                    cur_ty = match self
                        .wpool
                        .unwrap_perm(cur_ty, self.env, self.work, self.core, cur_span)?
                    {
                        WorkTypeNode::List(elem) | WorkTypeNode::Array(elem) => elem,
                        WorkTypeNode::Perm(TY_TEXT) => WTY_CHAR,
                        _ => {
                            return Err(self.type_err(
                                "expr.index",
                                cur_span,
                                "indexing requires list, array, or text",
                            ));
                        }
                    };
                    cur_span = ispan;
                }
                Ok(cur_ty)
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
            ExprKind::Let(l) => self.check_let_chain(l.as_ref(), expected),
            ExprKind::Fn(f) => self.check_fn_chain(f.as_ref(), expected, span),
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
                if self.opts.frozen {
                    Err(self.core.err_str(
                        "env.frozen",
                        ErrorKind::Parse,
                        span,
                        "def is only allowed at top level",
                    ))
                } else {
                    Err(self.core.err_str(
                        "expr.def",
                        ErrorKind::Parse,
                        span,
                        "def is only allowed at top level",
                    ))
                }
            }
            ExprKind::DefSchema(_) => {
                if self.opts.frozen {
                    Err(self.core.err_str(
                        "env.frozen",
                        ErrorKind::Parse,
                        span,
                        "defschema is only allowed at top level",
                    ))
                } else {
                    Err(self.core.err_str(
                        "type.defschema",
                        ErrorKind::Parse,
                        span,
                        "defschema is only allowed at top level",
                    ))
                }
            }
            ExprKind::Defn(_) => {
                if self.opts.frozen {
                    Err(self.core.err_str(
                        "env.frozen",
                        ErrorKind::Parse,
                        span,
                        "defn is only allowed at top level",
                    ))
                } else {
                    Err(self.core.err_str(
                        "expr.defn",
                        ErrorKind::Parse,
                        span,
                        "defn is only allowed at top level",
                    ))
                }
            }
            ExprKind::Import { .. } | ExprKind::Export { .. } => {
                if self.opts.frozen {
                    Err(self.core.err_str(
                        "env.frozen",
                        ErrorKind::Parse,
                        span,
                        "import/export is only allowed at top level",
                    ))
                } else {
                    Err(self.core.err_str(
                        "expr.module",
                        ErrorKind::Parse,
                        span,
                        "import/export is only allowed at top level",
                    ))
                }
            }
        }
    }
}
