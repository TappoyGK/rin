//! 組み込み関数（55 種）の型検査と高階関数・第一級組み込み参照の単一化。

use crate::ast::{Expr, ExprKind};
use crate::bytecode::BuiltinId;
use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::mem::RawVec;

use super::checker::TypeChecker;
use super::types::{TY_DATUM, TY_TEXT};
use super::work_types::{
    WTY_BOOL, WTY_CHAR, WTY_DATUM, WTY_F64, WTY_I64, WTY_IDENT, WTY_TEXT, WTY_VOID, WorkTypeId,
    WorkTypeNode, oom,
};

impl<'a> TypeChecker<'_, 'a> {
    /// 組み込み関数の直接呼び出しを型検査する。
    pub(super) fn check_builtin_call(
        &mut self,
        b: BuiltinId,
        args: &[Expr],
        expected: Option<WorkTypeId>,
        span: Span,
    ) -> Result<WorkTypeId, ErrRecorded> {
        self.wpool.tick(self.core, span)?;
        match b {
            // ── 算術演算 (`op.arith`) ───────────────────────────────
            BuiltinId::Add | BuiltinId::Mul => {
                if args.len() < 2 {
                    return Err(self.type_err(
                        "op.arith",
                        span,
                        "arithmetic operator requires at least 2 arguments",
                    ));
                }
                let t0 = self.check_expr(&args[0], expected)?;
                self.require_numeric(t0, args[0].span, "op.arith")?;
                for arg in &args[1..] {
                    let ti = self.check_expr(arg, Some(t0))?;
                    self.wpool
                        .unify(ti, t0, self.env, self.work, self.core, arg.span, "op.arith")?;
                }
                Ok(t0)
            }
            BuiltinId::Sub => {
                if args.is_empty() || args.len() > 2 {
                    return Err(self.type_err("op.arith", span, "'-' takes 1 or 2 arguments"));
                }
                let t0 = self.check_expr(&args[0], expected)?;
                self.require_numeric(t0, args[0].span, "op.arith")?;
                if args.len() == 2 {
                    let t1 = self.check_expr(&args[1], Some(t0))?;
                    self.wpool.unify(
                        t1,
                        t0,
                        self.env,
                        self.work,
                        self.core,
                        args[1].span,
                        "op.arith",
                    )?;
                }
                Ok(t0)
            }
            BuiltinId::Div | BuiltinId::Rem => {
                if args.len() != 2 {
                    return Err(self.type_err(
                        "op.arith",
                        span,
                        "division/remainder requires 2 arguments",
                    ));
                }
                let t0 = self.check_expr(&args[0], expected)?;
                self.require_numeric(t0, args[0].span, "op.arith")?;
                let t1 = self.check_expr(&args[1], Some(t0))?;
                self.wpool.unify(
                    t1,
                    t0,
                    self.env,
                    self.work,
                    self.core,
                    args[1].span,
                    "op.arith",
                )?;
                Ok(t0)
            }

            // ── 等値比較 (`op.cmp`) ─────────────────────────────────
            BuiltinId::Eq | BuiltinId::Ne => {
                if args.len() != 2 {
                    return Err(self.type_err(
                        "op.cmp",
                        span,
                        "equality operator requires 2 arguments",
                    ));
                }
                let t0 = self.check_expr(&args[0], None)?;
                let r0 = self.wpool.resolve(t0, self.core, args[0].span)?;
                if matches!(self.wpool.get(r0), WorkTypeNode::Perm(TY_DATUM)) {
                    let _ = self.check_expr(&args[1], Some(WTY_DATUM))?;
                    return Ok(WTY_BOOL);
                }
                let t1 = self.check_expr(&args[1], None)?;
                let r1 = self.wpool.resolve(t1, self.core, args[1].span)?;
                if matches!(self.wpool.get(r1), WorkTypeNode::Perm(TY_DATUM)) {
                    if matches!(args[0].kind, ExprKind::None)
                        || self
                            .wpool
                            .is_datum_compatible(r0, self.core, args[0].span)?
                    {
                        return Ok(WTY_BOOL);
                    }
                    return Err(self.type_err(
                        "op.cmp",
                        args[0].span,
                        "cannot compare this type with datum",
                    ));
                }
                self.wpool
                    .unify(r0, r1, self.env, self.work, self.core, span, "op.cmp")?;
                Ok(WTY_BOOL)
            }

            // ── 順序比較 (`op.cmp`) ─────────────────────────────────
            BuiltinId::Lt | BuiltinId::Le | BuiltinId::Gt | BuiltinId::Ge => {
                if args.len() != 2 {
                    return Err(self.type_err(
                        "op.cmp",
                        span,
                        "ordering operator requires 2 arguments",
                    ));
                }
                let t0 = self.check_expr(&args[0], None)?;
                self.require_orderable(t0, args[0].span, "op.cmp")?;
                let t1 = self.check_expr(&args[1], Some(t0))?;
                self.wpool.unify(
                    t1,
                    t0,
                    self.env,
                    self.work,
                    self.core,
                    args[1].span,
                    "op.cmp",
                )?;
                Ok(WTY_BOOL)
            }

            // ── 論理演算 (`op.logic`) ───────────────────────────────
            BuiltinId::And | BuiltinId::Or => {
                if args.is_empty() {
                    return Err(self.type_err(
                        "op.logic",
                        span,
                        "logical operator requires at least 1 argument",
                    ));
                }
                for arg in args {
                    let ti = self.check_expr(arg, Some(WTY_BOOL))?;
                    self.wpool.unify(
                        ti, WTY_BOOL, self.env, self.work, self.core, arg.span, "op.logic",
                    )?;
                }
                Ok(WTY_BOOL)
            }
            BuiltinId::Not => {
                if args.len() != 1 {
                    return Err(self.type_err("op.logic", span, "'not' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], Some(WTY_BOOL))?;
                self.wpool.unify(
                    t0,
                    WTY_BOOL,
                    self.env,
                    self.work,
                    self.core,
                    args[0].span,
                    "op.logic",
                )?;
                Ok(WTY_BOOL)
            }

            // ── 型変換 (`fn.convert`) ───────────────────────────────
            BuiltinId::AsF64 => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.convert", span, "'as_f64' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], None)?;
                self.require_numeric(t0, args[0].span, "fn.convert")?;
                Ok(WTY_F64)
            }
            BuiltinId::AsI64 => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.convert", span, "'as_i64' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], None)?;
                self.require_numeric(t0, args[0].span, "fn.convert")?;
                Ok(WTY_I64)
            }
            BuiltinId::IdentToText => {
                if args.len() != 1 {
                    return Err(self.type_err(
                        "fn.convert",
                        span,
                        "'ident->text' takes 1 argument",
                    ));
                }
                let t0 = self.check_expr(&args[0], Some(WTY_IDENT))?;
                self.wpool.unify(
                    t0,
                    WTY_IDENT,
                    self.env,
                    self.work,
                    self.core,
                    args[0].span,
                    "fn.convert",
                )?;
                Ok(WTY_TEXT)
            }
            BuiltinId::TextToIdent => {
                if args.len() != 1 {
                    return Err(self.type_err(
                        "fn.convert",
                        span,
                        "'text->ident' takes 1 argument",
                    ));
                }
                let t0 = self.check_expr(&args[0], Some(WTY_TEXT))?;
                self.wpool.unify(
                    t0,
                    WTY_TEXT,
                    self.env,
                    self.work,
                    self.core,
                    args[0].span,
                    "fn.convert",
                )?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::Option(WTY_IDENT), span)
            }

            // ── 数学関数 (`fn.math`) ────────────────────────────────
            BuiltinId::Sin
            | BuiltinId::Cos
            | BuiltinId::Sqrt
            | BuiltinId::Floor
            | BuiltinId::Ceil => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.math", span, "math function takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], Some(WTY_F64))?;
                self.wpool.unify(
                    t0,
                    WTY_F64,
                    self.env,
                    self.work,
                    self.core,
                    args[0].span,
                    "fn.math",
                )?;
                Ok(WTY_F64)
            }
            BuiltinId::Abs => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.math", span, "'abs' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], expected)?;
                self.require_numeric(t0, args[0].span, "fn.math")?;
                Ok(t0)
            }

            // ── リスト構築・参照 (`fn.list`) ────────────────────────
            BuiltinId::List => self.check_list_elements(args, expected, span),
            BuiltinId::Len => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.list", span, "'len' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], None)?;
                match self
                    .wpool
                    .unwrap_perm(t0, self.env, self.work, self.core, args[0].span)?
                {
                    WorkTypeNode::List(_)
                    | WorkTypeNode::Array(_)
                    | WorkTypeNode::Perm(TY_TEXT) => Ok(WTY_I64),
                    _ => Err(self.type_err(
                        "fn.list",
                        args[0].span,
                        "'len' expects list, array, or text",
                    )),
                }
            }
            BuiltinId::IsEmpty => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.list", span, "'is_empty' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], None)?;
                let _ = self.expect_list_elem(t0, args[0].span, "fn.list")?;
                Ok(WTY_BOOL)
            }
            BuiltinId::Get => {
                if args.len() != 2 {
                    return Err(self.type_err("fn.list", span, "'get' takes 2 arguments"));
                }
                let t0 = self.check_expr(&args[0], None)?;
                let ti = self.check_expr(&args[1], Some(WTY_I64))?;
                self.wpool.unify(
                    ti,
                    WTY_I64,
                    self.env,
                    self.work,
                    self.core,
                    args[1].span,
                    "fn.list",
                )?;
                let elem = match self.wpool.unwrap_perm(
                    t0,
                    self.env,
                    self.work,
                    self.core,
                    args[0].span,
                )? {
                    WorkTypeNode::List(e) | WorkTypeNode::Array(e) => e,
                    WorkTypeNode::Perm(TY_TEXT) => WTY_CHAR,
                    _ => {
                        return Err(self.type_err(
                            "fn.list",
                            args[0].span,
                            "'get' expects list, array, or text",
                        ));
                    }
                };
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::Option(elem), span)
            }
            BuiltinId::Car | BuiltinId::Last => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.list", span, "function takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], None)?;
                let elem = self.expect_list_elem(t0, args[0].span, "fn.list")?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::Option(elem), span)
            }
            BuiltinId::Cdr | BuiltinId::Reverse | BuiltinId::Unique => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.list", span, "function takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], expected)?;
                let elem = self.expect_list_elem(t0, args[0].span, "fn.list")?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(elem), span)
            }
            BuiltinId::Cons => {
                if args.len() != 2 {
                    return Err(self.type_err("fn.list", span, "'cons' takes 2 arguments"));
                }
                let exp_elem = self.expected_list_elem(expected, span)?;
                let t_list = self.check_expr(&args[1], expected)?;
                let elem = self.expect_list_elem(t_list, args[1].span, "fn.list")?;
                if let Some(ee) = exp_elem {
                    self.wpool
                        .unify(elem, ee, self.env, self.work, self.core, span, "fn.list")?;
                }
                let t_head = self.check_expr(&args[0], Some(elem))?;
                let _ = self.wpool.coerce_to_expected(
                    t_head,
                    elem,
                    self.env,
                    self.work,
                    self.core,
                    args[0].span,
                    "fn.list",
                )?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(elem), span)
            }
            BuiltinId::Push => {
                if args.len() != 2 {
                    return Err(self.type_err("fn.list", span, "'push' takes 2 arguments"));
                }
                let t_list = self.check_expr(&args[0], expected)?;
                let elem = self.expect_list_elem(t_list, args[0].span, "fn.list")?;
                let t_item = self.check_expr(&args[1], Some(elem))?;
                let _ = self.wpool.coerce_to_expected(
                    t_item,
                    elem,
                    self.env,
                    self.work,
                    self.core,
                    args[1].span,
                    "fn.list",
                )?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(elem), span)
            }
            BuiltinId::Append => {
                let elem = match self.expected_list_elem(expected, span)? {
                    Some(e) => e,
                    None => self.wpool.fresh_var(self.work, self.core, span)?,
                };
                let list_ty =
                    self.wpool
                        .alloc_node(self.work, self.core, WorkTypeNode::List(elem), span)?;
                for arg in args {
                    let ta = self.check_expr(arg, Some(list_ty))?;
                    let _ = self.wpool.coerce_to_expected(
                        ta, list_ty, self.env, self.work, self.core, arg.span, "fn.list",
                    )?;
                }
                Ok(list_ty)
            }
            BuiltinId::Flatten => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.list", span, "'flatten' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], None)?;
                let mid = self.expect_list_elem(t0, args[0].span, "fn.list")?;
                let inner = self.expect_list_elem(mid, args[0].span, "fn.list")?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(inner), span)
            }
            BuiltinId::Slice => {
                if args.len() != 3 {
                    return Err(self.type_err("fn.list", span, "'slice' takes 3 arguments"));
                }
                let t0 = self.check_expr(&args[0], expected)?;
                let t1 = self.check_expr(&args[1], Some(WTY_I64))?;
                let t2 = self.check_expr(&args[2], Some(WTY_I64))?;
                self.wpool.unify(
                    t1,
                    WTY_I64,
                    self.env,
                    self.work,
                    self.core,
                    args[1].span,
                    "fn.list",
                )?;
                self.wpool.unify(
                    t2,
                    WTY_I64,
                    self.env,
                    self.work,
                    self.core,
                    args[2].span,
                    "fn.list",
                )?;
                match self
                    .wpool
                    .unwrap_perm(t0, self.env, self.work, self.core, args[0].span)?
                {
                    WorkTypeNode::List(_) | WorkTypeNode::Perm(TY_TEXT) => Ok(t0),
                    _ => {
                        Err(self.type_err("fn.list", args[0].span, "'slice' expects list or text"))
                    }
                }
            }
            BuiltinId::Take | BuiltinId::Drop => {
                if args.len() != 2 {
                    return Err(self.type_err("fn.list", span, "function takes 2 arguments"));
                }
                let t_list = self.check_expr(&args[0], expected)?;
                let elem = self.expect_list_elem(t_list, args[0].span, "fn.list")?;
                let tn = self.check_expr(&args[1], Some(WTY_I64))?;
                self.wpool.unify(
                    tn,
                    WTY_I64,
                    self.env,
                    self.work,
                    self.core,
                    args[1].span,
                    "fn.list",
                )?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(elem), span)
            }
            BuiltinId::Range => {
                if args.len() != 2 {
                    return Err(self.type_err("fn.list", span, "'range' takes 2 arguments"));
                }
                let t0 = self.check_expr(&args[0], Some(WTY_I64))?;
                let t1 = self.check_expr(&args[1], Some(WTY_I64))?;
                self.wpool.unify(
                    t0,
                    WTY_I64,
                    self.env,
                    self.work,
                    self.core,
                    args[0].span,
                    "fn.list",
                )?;
                self.wpool.unify(
                    t1,
                    WTY_I64,
                    self.env,
                    self.work,
                    self.core,
                    args[1].span,
                    "fn.list",
                )?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(WTY_I64), span)
            }
            BuiltinId::Contains => {
                if args.len() != 2 {
                    return Err(self.type_err("fn.list", span, "'contains' takes 2 arguments"));
                }
                let t_list = self.check_expr(&args[0], None)?;
                let elem = self.expect_list_elem(t_list, args[0].span, "fn.list")?;
                let t_val = self.check_expr(&args[1], Some(elem))?;
                let _ = self.wpool.coerce_to_expected(
                    t_val,
                    elem,
                    self.env,
                    self.work,
                    self.core,
                    args[1].span,
                    "fn.list",
                )?;
                Ok(WTY_BOOL)
            }
            BuiltinId::Zip => {
                if args.len() != 2 {
                    return Err(self.type_err("fn.list", span, "'zip' takes 2 arguments"));
                }
                let ta = self.check_expr(&args[0], None)?;
                let tb = self.check_expr(&args[1], None)?;
                let ea = self.expect_list_elem(ta, args[0].span, "fn.list")?;
                let eb = self.expect_list_elem(tb, args[1].span, "fn.list")?;
                let mut pair = RawVec::new();
                if pair.push_work(self.work, self.core, ea).is_none()
                    || pair.push_work(self.work, self.core, eb).is_none()
                {
                    return Err(oom(self.core, span));
                }
                let tup =
                    self.wpool
                        .alloc_node(self.work, self.core, WorkTypeNode::Tuple(pair), span)?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(tup), span)
            }
            BuiltinId::Enumerate => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.list", span, "'enumerate' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], None)?;
                let elem = self.expect_list_elem(t0, args[0].span, "fn.list")?;
                let mut pair = RawVec::new();
                if pair.push_work(self.work, self.core, WTY_I64).is_none()
                    || pair.push_work(self.work, self.core, elem).is_none()
                {
                    return Err(oom(self.core, span));
                }
                let tup =
                    self.wpool
                        .alloc_node(self.work, self.core, WorkTypeNode::Tuple(pair), span)?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(tup), span)
            }
            BuiltinId::Sort => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.list", span, "'sort' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], expected)?;
                let elem = self.expect_list_elem(t0, args[0].span, "fn.list")?;
                self.require_orderable(elem, args[0].span, "fn.list")?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(elem), span)
            }
            BuiltinId::ListToArray => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.list", span, "'list->array' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], None)?;
                let elem = self.expect_list_elem(t0, args[0].span, "fn.list")?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::Array(elem), span)
            }
            BuiltinId::ArrayToList => {
                if args.len() != 1 {
                    return Err(self.type_err("fn.list", span, "'array->list' takes 1 argument"));
                }
                let t0 = self.check_expr(&args[0], None)?;
                let elem = match self.wpool.unwrap_perm(
                    t0,
                    self.env,
                    self.work,
                    self.core,
                    args[0].span,
                )? {
                    WorkTypeNode::Array(e) => e,
                    _ => {
                        return Err(self.type_err(
                            "fn.list",
                            args[0].span,
                            "'array->list' expects array",
                        ));
                    }
                };
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(elem), span)
            }

            // ── 高階組み込み関数（リスト・初期値を先に検査しコールバックへ期待型伝播） ──
            BuiltinId::Find | BuiltinId::Any | BuiltinId::All | BuiltinId::Filter => {
                if args.len() != 2 {
                    return Err(self.type_err(
                        "fn.list",
                        span,
                        "higher-order function takes 2 arguments",
                    ));
                }
                let list_exp = if matches!(b, BuiltinId::Filter) {
                    expected
                } else {
                    None
                };
                let t_list = self.check_expr(&args[1], list_exp)?;
                let elem = self.expect_list_elem(t_list, args[1].span, "fn.list")?;
                let exp_fn = self.make_fn_type(&[elem], WTY_BOOL, span)?;
                let tf = self.check_expr(&args[0], Some(exp_fn))?;
                self.unify_callback(tf, exp_fn, args[0].span, "fn.list")?;
                match b {
                    BuiltinId::Find => self.wpool.alloc_node(
                        self.work,
                        self.core,
                        WorkTypeNode::Option(elem),
                        span,
                    ),
                    BuiltinId::Any | BuiltinId::All => Ok(WTY_BOOL),
                    BuiltinId::Filter => {
                        self.wpool
                            .alloc_node(self.work, self.core, WorkTypeNode::List(elem), span)
                    }
                    _ => unreachable!(),
                }
            }
            BuiltinId::Map => {
                if args.len() != 2 {
                    return Err(self.type_err("fn.list", span, "'map' takes 2 arguments"));
                }
                let t_list = self.check_expr(&args[1], None)?;
                let elem_a = self.expect_list_elem(t_list, args[1].span, "fn.list")?;
                let elem_b = match self.expected_list_elem(expected, span)? {
                    Some(eb) => eb,
                    None => self.wpool.fresh_var(self.work, self.core, span)?,
                };
                let exp_fn = self.make_fn_type(&[elem_a], elem_b, span)?;
                let tf = self.check_expr(&args[0], Some(exp_fn))?;
                self.unify_callback(tf, exp_fn, args[0].span, "fn.list")?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(elem_b), span)
            }
            BuiltinId::FlatMap => {
                if args.len() != 2 {
                    return Err(self.type_err("fn.list", span, "'flat_map' takes 2 arguments"));
                }
                let t_list = self.check_expr(&args[1], None)?;
                let elem_a = self.expect_list_elem(t_list, args[1].span, "fn.list")?;
                let elem_b = match self.expected_list_elem(expected, span)? {
                    Some(eb) => eb,
                    None => self.wpool.fresh_var(self.work, self.core, span)?,
                };
                let ret_list = self.wpool.alloc_node(
                    self.work,
                    self.core,
                    WorkTypeNode::List(elem_b),
                    span,
                )?;
                let exp_fn = self.make_fn_type(&[elem_a], ret_list, span)?;
                let tf = self.check_expr(&args[0], Some(exp_fn))?;
                self.unify_callback(tf, exp_fn, args[0].span, "fn.list")?;
                Ok(ret_list)
            }
            BuiltinId::Fold => {
                if args.len() != 3 {
                    return Err(self.type_err("fn.list", span, "'fold' takes 3 arguments"));
                }
                let t_init = self.check_expr(&args[1], expected)?;
                let t_list = self.check_expr(&args[2], None)?;
                let elem = self.expect_list_elem(t_list, args[2].span, "fn.list")?;
                let exp_fn = self.make_fn_type(&[t_init, elem], t_init, span)?;
                let tf = self.check_expr(&args[0], Some(exp_fn))?;
                self.unify_callback(tf, exp_fn, args[0].span, "fn.list")?;
                Ok(t_init)
            }
            BuiltinId::SortBy => {
                if args.len() != 2 {
                    return Err(self.type_err("fn.list", span, "'sort_by' takes 2 arguments"));
                }
                let t_list = self.check_expr(&args[1], expected)?;
                let elem = self.expect_list_elem(t_list, args[1].span, "fn.list")?;
                let key_ty = self.wpool.fresh_var(self.work, self.core, span)?;
                let exp_fn = self.make_fn_type(&[elem], key_ty, span)?;
                let tf = self.check_expr(&args[0], Some(exp_fn))?;
                self.unify_callback(tf, exp_fn, args[0].span, "fn.list")?;
                self.require_orderable(key_ty, args[0].span, "fn.list")?;
                self.wpool
                    .alloc_node(self.work, self.core, WorkTypeNode::List(elem), span)
            }
        }
    }

    /// コールバック位置（例: `(map not xs)`, `(fold + 0 xs)`）で第一級組み込み関数が渡された場合に
    /// 期待関数型 `exp_fn` と照合する。
    pub(super) fn unify_callback(
        &mut self,
        actual: WorkTypeId,
        exp_fn: WorkTypeId,
        span: Span,
        rule: &'static str,
    ) -> Result<(), ErrRecorded> {
        let act_r = self.wpool.resolve(actual, self.core, span)?;
        if let WorkTypeNode::Builtin(b) = self.wpool.get(act_r) {
            return self.instantiate_builtin_as_fn(b, exp_fn, span, rule);
        }
        self.wpool
            .unify(act_r, exp_fn, self.env, self.work, self.core, span, rule)
    }

    /// 第一級組み込み関数 `b` を期待関数型 `exp_fn` に合わせてインスタンス化・検証する。
    pub(super) fn instantiate_builtin_as_fn(
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

    pub(super) fn check_test_builtin_call(
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

    pub(super) fn require_numeric(
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

    pub(super) fn require_orderable(
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

    pub(super) fn expect_list_elem(
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

    pub(super) fn expected_list_elem(
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
            _ => Ok(None),
        }
    }

    pub(super) fn make_fn_type(
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
    pub(super) fn type_err(&mut self, rule: &'static str, span: Span, msg: &str) -> ErrRecorded {
        self.core.err_str(rule, ErrorKind::Type, span, msg)
    }
}
