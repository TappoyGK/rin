//! `match` 式のパターン検査・網羅性・到達不能枝の検出。

use super::*;

impl<'a> TypeChecker<'a, '_> {
    pub(super) fn check_match(
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
}
