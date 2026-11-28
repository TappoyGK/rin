//! 第一級組み込み関数のインスタンス化、テスト用組み込み関数、および共通型ヘルパ。

use super::*;

impl<'a> TypeChecker<'_, 'a> {
    /// 第一級組み込み関数 `b` を期待関数型 `exp_fn` に合わせてインスタンス化・検証する。
    pub(in crate::typecheck) fn instantiate_builtin_as_fn(
        &mut self,
        b: BuiltinId,
        exp_fn: WorkTypeId,
        span: Span,
        rule: &'static str,
    ) -> Result<(), ErrRecorded> {
        let WorkTypeNode::Fn { params, ret } = self
            .wpool
            .unwrap_perm(exp_fn, self.env, self.work, self.core, span)?
        else {
            return Err(self.type_err(rule, span, "expected function type for builtin"));
        };
        let ps = params.as_slice();
        match b {
            BuiltinId::Add | BuiltinId::Sub | BuiltinId::Mul | BuiltinId::Div | BuiltinId::Rem => {
                if (b == BuiltinId::Sub && ps.len() == 1) || ps.len() == 2 {
                    self.require_numeric(ps[0], span, rule)?;
                    for &p in &ps[1..] {
                        self.wpool
                            .unify(p, ps[0], self.env, self.work, self.core, span, rule)?;
                    }
                    self.wpool
                        .unify(ret, ps[0], self.env, self.work, self.core, span, rule)
                } else {
                    Err(self.type_err(rule, span, "builtin arity mismatch"))
                }
            }
            BuiltinId::Not => {
                let [p0] = ps else {
                    return Err(self.type_err(rule, span, "'not' expects 1 argument"));
                };
                self.wpool
                    .unify(*p0, WTY_BOOL, self.env, self.work, self.core, span, rule)?;
                self.wpool
                    .unify(ret, WTY_BOOL, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::And | BuiltinId::Or => {
                if ps.is_empty() {
                    return Err(self.type_err(rule, span, "logical builtin expects >= 1 arg"));
                }
                for &p in ps {
                    self.wpool
                        .unify(p, WTY_BOOL, self.env, self.work, self.core, span, rule)?;
                }
                self.wpool
                    .unify(ret, WTY_BOOL, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::AsF64 => {
                let [p0] = ps else {
                    return Err(self.type_err(rule, span, "'as_f64' expects 1 argument"));
                };
                self.require_numeric(*p0, span, rule)?;
                self.wpool
                    .unify(ret, WTY_F64, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::AsI64 => {
                let [p0] = ps else {
                    return Err(self.type_err(rule, span, "'as_i64' expects 1 argument"));
                };
                self.require_numeric(*p0, span, rule)?;
                self.wpool
                    .unify(ret, WTY_I64, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::Sin
            | BuiltinId::Cos
            | BuiltinId::Sqrt
            | BuiltinId::Floor
            | BuiltinId::Ceil => {
                let [p0] = ps else {
                    return Err(self.type_err(rule, span, "math function expects 1 argument"));
                };
                self.wpool
                    .unify(*p0, WTY_F64, self.env, self.work, self.core, span, rule)?;
                self.wpool
                    .unify(ret, WTY_F64, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::Abs => {
                let [p0] = ps else {
                    return Err(self.type_err(rule, span, "'abs' expects 1 argument"));
                };
                self.require_numeric(*p0, span, rule)?;
                self.wpool
                    .unify(ret, *p0, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::IdentToText => {
                let [p0] = ps else {
                    return Err(self.type_err(rule, span, "'ident->text' expects 1 argument"));
                };
                self.wpool
                    .unify(*p0, WTY_IDENT, self.env, self.work, self.core, span, rule)?;
                self.wpool
                    .unify(ret, WTY_TEXT, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::TextToIdent => {
                let [p0] = ps else {
                    return Err(self.type_err(rule, span, "'text->ident' expects 1 argument"));
                };
                self.wpool
                    .unify(*p0, WTY_TEXT, self.env, self.work, self.core, span, rule)?;
                let opt = self.wpool.alloc_node(
                    self.work,
                    self.core,
                    WorkTypeNode::Option(WTY_IDENT),
                    span,
                )?;
                self.wpool
                    .unify(ret, opt, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::Len => {
                let [p0] = ps else {
                    return Err(self.type_err(rule, span, "'len' expects 1 argument"));
                };
                match self
                    .wpool
                    .unwrap_perm(*p0, self.env, self.work, self.core, span)?
                {
                    WorkTypeNode::List(_)
                    | WorkTypeNode::Array(_)
                    | WorkTypeNode::Perm(TY_TEXT) => self
                        .wpool
                        .unify(ret, WTY_I64, self.env, self.work, self.core, span, rule),
                    _ => Err(self.type_err(rule, span, "'len' expects list, array, or text")),
                }
            }
            BuiltinId::IsEmpty => {
                let [p0] = ps else {
                    return Err(self.type_err(rule, span, "'is_empty' expects 1 argument"));
                };
                let _ = self.expect_list_elem(*p0, span, rule)?;
                self.wpool
                    .unify(ret, WTY_BOOL, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::Car | BuiltinId::Last => {
                let [p0] = ps else {
                    return Err(self.type_err(rule, span, "function expects 1 argument"));
                };
                let elem = self.expect_list_elem(*p0, span, rule)?;
                let opt = self.wpool.alloc_node(
                    self.work,
                    self.core,
                    WorkTypeNode::Option(elem),
                    span,
                )?;
                self.wpool
                    .unify(ret, opt, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::Cdr | BuiltinId::Reverse | BuiltinId::Unique => {
                let [p0] = ps else {
                    return Err(self.type_err(rule, span, "function expects 1 argument"));
                };
                let elem = self.expect_list_elem(*p0, span, rule)?;
                let lst =
                    self.wpool
                        .alloc_node(self.work, self.core, WorkTypeNode::List(elem), span)?;
                self.wpool
                    .unify(ret, lst, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::Eq | BuiltinId::Ne => {
                let [p0, p1] = ps else {
                    return Err(self.type_err(rule, span, "equality expects 2 arguments"));
                };
                self.wpool
                    .unify(*p0, *p1, self.env, self.work, self.core, span, rule)?;
                self.wpool
                    .unify(ret, WTY_BOOL, self.env, self.work, self.core, span, rule)
            }
            BuiltinId::Lt | BuiltinId::Le | BuiltinId::Gt | BuiltinId::Ge => {
                let [p0, p1] = ps else {
                    return Err(self.type_err(rule, span, "ordering expects 2 arguments"));
                };
                self.require_orderable(*p0, span, rule)?;
                self.wpool
                    .unify(*p0, *p1, self.env, self.work, self.core, span, rule)?;
                self.wpool
                    .unify(ret, WTY_BOOL, self.env, self.work, self.core, span, rule)
            }
            _ => Err(self.type_err(
                rule,
                span,
                "cannot use this polymorphic builtin as a first-class value without a wrapper fn",
            )),
        }
    }

    pub(in crate::typecheck) fn check_test_builtin_call(
        &mut self,
        name: &str,
        args: &[Expr],
        span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        match name {
            "deftest" => {
                if args.len() < 2 {
                    return Err(self.type_err(
                        "test.deftest",
                        span,
                        "'deftest' requires name and body",
                    ));
                }
                let t_name = self.check_expr(&args[0], Some(WTY_TEXT))?;
                self.wpool.unify(
                    t_name,
                    WTY_TEXT,
                    self.env,
                    self.work,
                    self.core,
                    args[0].span,
                    "test.deftest",
                )?;
                for body_expr in &args[1..] {
                    let _ = self.check_expr(body_expr, None)?;
                }
                Ok(WTY_VOID)
            }
            "check" => {
                if args.len() != 1 {
                    return Err(self.type_err("test.check", span, "'check' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], Some(WTY_BOOL))?;
                self.wpool.unify(
                    t0,
                    WTY_BOOL,
                    self.env,
                    self.work,
                    self.core,
                    args[0].span,
                    "test.check",
                )?;
                Ok(WTY_VOID)
            }
            "check_eq" => {
                if args.len() != 2 {
                    return Err(self.type_err("test.check", span, "'check_eq' takes 2 arguments"));
                }
                let t0 = self.check_expr(&args[0], None)?;
                let t1 = self.check_expr(&args[1], Some(t0))?;
                self.wpool
                    .unify(t0, t1, self.env, self.work, self.core, span, "test.check")?;
                Ok(WTY_VOID)
            }
            _ => unreachable!(),
        }
    }

    pub(in crate::typecheck) fn require_numeric(
        &mut self,
        ty: WorkTypeId,
        span: Span,
        rule: &'static str,
    ) -> Result<(), ErrRecorded> {
        if !self.wpool.is_numeric(ty, self.core, span)? {
            return Err(self.type_err(rule, span, "expected i64 or f64"));
        }
        Ok(())
    }

    pub(in crate::typecheck) fn require_orderable(
        &mut self,
        ty: WorkTypeId,
        span: Span,
        rule: &'static str,
    ) -> Result<(), ErrRecorded> {
        if !self.wpool.is_orderable(ty, self.core, span)? {
            return Err(self.type_err(
                rule,
                span,
                "expected orderable type (i64, f64, text, or ident)",
            ));
        }
        Ok(())
    }

    pub(in crate::typecheck) fn expect_list_elem(
        &mut self,
        ty: WorkTypeId,
        span: Span,
        rule: &'static str,
    ) -> Result<WorkTypeId, ErrRecorded> {
        match self
            .wpool
            .unwrap_perm(ty, self.env, self.work, self.core, span)?
        {
            WorkTypeNode::List(elem) => Ok(elem),
            WorkTypeNode::Var(_) => {
                let elem = self.wpool.fresh_var(self.work, self.core, span)?;
                let list_ty =
                    self.wpool
                        .alloc_node(self.work, self.core, WorkTypeNode::List(elem), span)?;
                self.wpool
                    .unify(ty, list_ty, self.env, self.work, self.core, span, rule)?;
                Ok(elem)
            }
            _ => Err(self.type_err(rule, span, "expected list")),
        }
    }

    pub(in crate::typecheck) fn expected_list_elem(
        &mut self,
        expected: Option<WorkTypeId>,
        span: Span,
    ) -> Result<Option<WorkTypeId>, ErrRecorded> {
        let Some(exp) = expected else {
            return Ok(None);
        };
        match self
            .wpool
            .unwrap_perm(exp, self.env, self.work, self.core, span)?
        {
            WorkTypeNode::List(elem) => Ok(Some(elem)),
            WorkTypeNode::Perm(TY_DATUM) => Ok(Some(WTY_DATUM)),
            _ => Ok(None),
        }
    }

    pub(in crate::typecheck) fn make_fn_type(
        &mut self,
        params: &[WorkTypeId],
        ret: WorkTypeId,
        span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        let mut pvec = RawVec::new();
        for &p in params {
            if pvec.push_work(self.work, self.core, p).is_none() {
                return Err(oom(self.core, span));
            }
        }
        self.wpool.alloc_node(
            self.work,
            self.core,
            WorkTypeNode::Fn { params: pvec, ret },
            span,
        )
    }

    #[inline]
    pub(in crate::typecheck) fn type_err(
        &mut self,
        rule: &'static str,
        span: Span,
        msg: &str,
    ) -> ErrRecorded {
        self.core.err_str(rule, ErrorKind::Type, span, msg)
    }
}
