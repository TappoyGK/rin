//! リスト補助、同値・順序比較、算術・数学演算を実装する。

use crate::bytecode::BuiltinId;
use crate::error::{ErrRecorded, Span};
use crate::pool::{Obj, RVal};

use super::Runner;

mod equality;

impl Runner<'_, '_> {
    pub(super) fn is_list(&self, v: RVal) -> bool {
        matches!(v, RVal::EmptyList)
            || matches!(v, RVal::Obj(id) if matches!(self.pool.get(id), Obj::Cons { .. }))
    }

    pub(super) fn list_head(&self, v: RVal) -> Option<RVal> {
        match v {
            RVal::EmptyList => None,
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Cons { head, .. } => Some(*head),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn list_tail(&self, v: RVal) -> RVal {
        match v {
            RVal::EmptyList => RVal::EmptyList,
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Cons { tail, .. } => *tail,
                _ => RVal::EmptyList,
            },
            _ => RVal::EmptyList,
        }
    }

    pub(super) fn list_get(
        &mut self,
        mut v: RVal,
        mut idx: usize,
        span: Span,
    ) -> Result<Option<RVal>, ErrRecorded> {
        if !self.try_charge(idx.saturating_add(1).min(self.pool.list_len(v))) {
            return self.step_limit_result(span);
        }
        while let RVal::Obj(id) = v {
            let Obj::Cons { head, tail, .. } = self.pool.get(id) else {
                break;
            };
            if idx == 0 {
                return Ok(Some(*head));
            }
            idx -= 1;
            v = *tail;
        }
        Ok(None)
    }

    pub(super) fn list_to_work_vec(
        &mut self,
        mut v: RVal,
        span: Span,
    ) -> Result<crate::mem::RawVec<RVal>, ErrRecorded> {
        let len = self.pool.list_len(v);
        if !self.try_charge(len) {
            return self.step_limit_result(span);
        }
        let mut out = crate::mem::RawVec::new();
        if out
            .reserve_work(&mut self.pool.work, &mut self.pool.core, len)
            .is_none()
        {
            return Err(self.work_oom(span));
        }
        while let RVal::Obj(id) = v {
            let Obj::Cons { head, tail, .. } = *self.pool.get(id) else {
                break;
            };
            let _ = out.push_work(&mut self.pool.work, &mut self.pool.core, head);
            v = tail;
        }
        Ok(out)
    }

    pub(super) fn build_list_from_owned_raw(
        &mut self,
        items: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        if !self.try_charge(items.len()) {
            self.release_slice(items);
            return self.step_limit_result(span);
        }
        let mut acc = RVal::EmptyList;
        for (idx, &item) in items.iter().enumerate().rev() {
            match self.pool.alloc_cons(item, acc, span) {
                Ok(next) => {
                    self.pool.release_val(acc);
                    acc = next;
                }
                Err(e) => {
                    self.pool.release_val(acc);
                    for &rem in &items[..=idx] {
                        self.pool.release_val(rem);
                    }
                    return Err(e);
                }
            }
            self.pool.release_val(item);
        }
        Ok(acc)
    }

    pub(super) fn build_list_from_owned_slice(
        &mut self,
        start: usize,
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let mut acc = RVal::EmptyList;
        let end = self.stack.len();
        if !self.try_charge(end - start) {
            self.release_stack_from(start);
            return self.step_limit_result(span);
        }
        for idx in (start..end).rev() {
            let item = self.stack.as_slice()[idx];
            match self.pool.alloc_cons(item, acc, span) {
                Ok(next) => {
                    self.pool.release_val(acc);
                    acc = next;
                }
                Err(e) => {
                    self.pool.release_val(acc);
                    self.release_stack_from(start);
                    return Err(e);
                }
            }
            self.pool.release_val(item);
            self.stack.pop();
        }
        Ok(acc)
    }

    pub(super) fn build_list_from_borrowed(
        &mut self,
        items: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        if !self.try_charge(items.len()) {
            return self.step_limit_result(span);
        }
        let mut acc = RVal::EmptyList;
        for &item in items.iter().rev() {
            match self.pool.alloc_cons(item, acc, span) {
                Ok(next) => {
                    self.pool.release_val(acc);
                    acc = next;
                }
                Err(e) => {
                    self.pool.release_val(acc);
                    return Err(e);
                }
            }
        }
        Ok(acc)
    }

    pub(super) fn expect_list(&mut self, op: &str, v: RVal, span: Span) -> Result<(), ErrRecorded> {
        if self.is_list(v) {
            Ok(())
        } else {
            let tname = v.type_name(self.pool);
            Err(self.err_runtime_fmt(
                "fn.list",
                span,
                format_args!("'{op}' expects list, got {tname}"),
            ))
        }
    }

    pub(super) fn expect_one_list(
        &mut self,
        op: &str,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let [v] = args else {
            return Err(self.err_runtime_fmt(
                "fn.list",
                span,
                format_args!("'{op}' takes 1 argument"),
            ));
        };
        self.expect_list(op, *v, span)?;
        Ok(*v)
    }

    pub(super) fn expect_int(&mut self, op: &str, v: RVal, span: Span) -> Result<i64, ErrRecorded> {
        match v {
            RVal::Int(n) => Ok(n),
            other => {
                let tname = other.type_name(self.pool);
                Err(self.err_runtime_fmt(
                    "fn.list",
                    span,
                    format_args!("'{op}' expects i64, got {tname}"),
                ))
            }
        }
    }

    // 標準ソートの比較回数には依存せず、最悪時のオーダーで前払いする。
    pub(super) fn charge_sort(&mut self, keys: impl Iterator<Item = RVal>, n: usize) -> bool {
        if n < 2 {
            return true;
        }
        let bytes = keys
            .map(|key| match key {
                RVal::Obj(id) => match self.pool.get(id) {
                    Obj::Str(s) | Obj::Ident(s) => s.len(),
                    _ => 0,
                },
                _ => 0,
            })
            .max()
            .unwrap_or(0);
        let log = (usize::BITS - (n - 1).leading_zeros()) as usize;
        let Some(cost) = bytes
            .checked_add(1)
            .and_then(|width| n.checked_mul(log)?.checked_mul(width))
        else {
            return false;
        };
        self.try_charge(cost)
    }

    #[inline(always)]
    pub(super) fn compare_vals(
        &mut self,
        a: RVal,
        b: RVal,
        span: Span,
    ) -> Result<std::cmp::Ordering, ErrRecorded> {
        match (a, b) {
            (RVal::Int(x), RVal::Int(y)) => Ok(x.cmp(&y)),
            (RVal::Float(x), RVal::Float(y)) => match x.partial_cmp(&y) {
                Some(ord) => Ok(ord),
                None => Err(self.compare_nan_err(span)),
            },
            (RVal::Char(x), RVal::Char(y)) => Ok(x.cmp(&y)),
            (RVal::Obj(id1), RVal::Obj(id2)) => match (self.pool.get(id1), self.pool.get(id2)) {
                (Obj::Str(s1), Obj::Str(s2)) | (Obj::Ident(s1), Obj::Ident(s2)) => {
                    Ok(s1.as_str().cmp(s2.as_str()))
                }
                _ => Err(self.compare_type_err(a, b, span)),
            },
            _ => Err(self.compare_type_err(a, b, span)),
        }
    }

    #[cold]
    #[inline(never)]
    fn compare_nan_err(&mut self, span: Span) -> ErrRecorded {
        self.err_runtime("fn.list", span, "cannot order NaN values")
    }

    #[cold]
    #[inline(never)]
    fn compare_type_err(&mut self, a: RVal, b: RVal, span: Span) -> ErrRecorded {
        let ta = a.type_name(self.pool);
        let tb = b.type_name(self.pool);
        self.err_runtime_fmt("fn.list", span, format_args!("cannot order {ta} and {tb}"))
    }

    pub(super) fn arith(
        &mut self,
        op: BuiltinId,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        if op == BuiltinId::Sub && args.len() == 1 {
            return match args[0] {
                RVal::Int(a) => match a.checked_neg() {
                    Some(v) => Ok(RVal::Int(v)),
                    None => Err(self.err_runtime("op.arith", span, "i64 overflow")),
                },
                RVal::Float(a) => Ok(RVal::Float(-a)),
                other => {
                    let tname = other.type_name(self.pool);
                    Err(self.err_runtime_fmt(
                        "op.arith",
                        span,
                        format_args!("cannot negate {tname}"),
                    ))
                }
            };
        }
        if (op == BuiltinId::Add || op == BuiltinId::Mul) && args.len() >= 2 {
            if !self.try_charge(args.len() - 2) {
                return self.step_limit_result(span);
            }
            let mut acc = args[0];
            for &rhs in &args[1..] {
                acc = self.arith_bin(op, acc, rhs, span)?;
            }
            return Ok(acc);
        }
        let [a, b] = args else {
            return Err(self.err_runtime_fmt(
                "op.arith",
                span,
                format_args!("'{}' takes 2 arguments", op.name()),
            ));
        };
        self.arith_bin(op, *a, *b, span)
    }

    pub(super) fn arith_bin(
        &mut self,
        op: BuiltinId,
        a: RVal,
        b: RVal,
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        match (a, b) {
            (RVal::Int(x), RVal::Int(y)) => {
                let res = match op {
                    BuiltinId::Add => x.checked_add(y),
                    BuiltinId::Sub => x.checked_sub(y),
                    BuiltinId::Mul => x.checked_mul(y),
                    BuiltinId::Div => {
                        if y == 0 {
                            return Err(self.err_runtime("op.arith", span, "division by zero"));
                        }
                        x.checked_div(y)
                    }
                    BuiltinId::Rem => {
                        if y == 0 {
                            return Err(self.err_runtime("op.arith", span, "division by zero"));
                        }
                        x.checked_rem(y)
                    }
                    _ => unreachable!(),
                };
                match res {
                    Some(v) => Ok(RVal::Int(v)),
                    None => Err(self.err_runtime("op.arith", span, "i64 overflow")),
                }
            }
            (RVal::Float(x), RVal::Float(y)) => match op {
                BuiltinId::Add => Ok(RVal::Float(x + y)),
                BuiltinId::Sub => Ok(RVal::Float(x - y)),
                BuiltinId::Mul => Ok(RVal::Float(x * y)),
                BuiltinId::Div => Ok(RVal::Float(x / y)),
                BuiltinId::Rem => Ok(RVal::Float(x % y)),
                _ => unreachable!(),
            },
            _ => {
                let ta = a.type_name(self.pool);
                let tb = b.type_name(self.pool);
                Err(self.err_runtime_fmt(
                    "op.arith",
                    span,
                    format_args!(
                        "cannot apply '{}' to {ta} and {tb} (no implicit conversion)",
                        op.name()
                    ),
                ))
            }
        }
    }

    pub(super) fn eq_cmp(
        &mut self,
        want_eq: bool,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let [a, b] = args else {
            return Err(self.err_runtime("op.cmp", span, "comparison takes 2 arguments"));
        };
        let ta = a.type_name(self.pool);
        let tb = b.type_name(self.pool);
        if ta != tb {
            return Err(self.err_runtime_fmt(
                "op.cmp",
                span,
                format_args!("cannot compare {ta} and {tb}"),
            ));
        }
        let Some(eq) = self.values_equal(*a, *b) else {
            return self.step_limit_result(span);
        };
        Ok(RVal::Bool(if want_eq { eq } else { !eq }))
    }

    pub(super) fn ord_cmp(
        &mut self,
        op: BuiltinId,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let [a, b] = args else {
            return Err(self.err_runtime_fmt(
                "op.cmp",
                span,
                format_args!("'{}' takes 2 arguments", op.name()),
            ));
        };
        let res = match (*a, *b) {
            (RVal::Int(x), RVal::Int(y)) => match op {
                BuiltinId::Lt => x < y,
                BuiltinId::Le => x <= y,
                BuiltinId::Gt => x > y,
                BuiltinId::Ge => x >= y,
                _ => unreachable!(),
            },
            (RVal::Float(x), RVal::Float(y)) => match op {
                BuiltinId::Lt => x < y,
                BuiltinId::Le => x <= y,
                BuiltinId::Gt => x > y,
                BuiltinId::Ge => x >= y,
                _ => unreachable!(),
            },
            (RVal::Obj(id1), RVal::Obj(id2)) => {
                let cmp = match (self.pool.get(id1), self.pool.get(id2)) {
                    (Obj::Str(s1), Obj::Str(s2)) | (Obj::Ident(s1), Obj::Ident(s2)) => {
                        if id1 == id2 {
                            Some((0, std::cmp::Ordering::Equal))
                        } else {
                            let cost = s1.len().min(s2.len());
                            let ord = s1.as_str().cmp(s2.as_str());
                            Some((cost, ord))
                        }
                    }
                    _ => None,
                };
                let Some((cost, ord)) = cmp else {
                    let ta = a.type_name(self.pool);
                    let tb = b.type_name(self.pool);
                    return Err(self.err_runtime_fmt(
                        "op.cmp",
                        span,
                        format_args!("cannot compare {ta} and {tb}"),
                    ));
                };
                if !self.try_charge(cost) {
                    return self.step_limit_result(span);
                }
                match op {
                    BuiltinId::Lt => ord.is_lt(),
                    BuiltinId::Le => ord.is_le(),
                    BuiltinId::Gt => ord.is_gt(),
                    BuiltinId::Ge => ord.is_ge(),
                    _ => unreachable!(),
                }
            }
            _ => {
                let ta = a.type_name(self.pool);
                let tb = b.type_name(self.pool);
                return Err(self.err_runtime_fmt(
                    "op.cmp",
                    span,
                    format_args!("cannot compare {ta} and {tb}"),
                ));
            }
        };
        Ok(RVal::Bool(res))
    }

    pub(super) fn math(
        &mut self,
        b: BuiltinId,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let [v] = args else {
            return Err(self.err_runtime_fmt(
                "fn.math",
                span,
                format_args!("'{}' takes 1 argument", b.name()),
            ));
        };
        if b == BuiltinId::Abs {
            return match v {
                RVal::Int(n) => match n.checked_abs() {
                    Some(x) => Ok(RVal::Int(x)),
                    None => Err(self.err_runtime("fn.math", span, "i64 overflow")),
                },
                RVal::Float(x) => Ok(RVal::Float(x.abs())),
                other => {
                    let tname = other.type_name(self.pool);
                    Err(self.err_runtime_fmt(
                        "fn.math",
                        span,
                        format_args!("'abs' expects number, got {tname}"),
                    ))
                }
            };
        }
        let RVal::Float(x) = v else {
            let tname = v.type_name(self.pool);
            return Err(self.err_runtime_fmt(
                "fn.math",
                span,
                format_args!("'{}' expects f64, got {tname}", b.name()),
            ));
        };
        let res = match b {
            BuiltinId::Sin => crate::math::sin(*x, self.math_fma),
            BuiltinId::Cos => crate::math::cos(*x, self.math_fma),
            BuiltinId::Sqrt => x.sqrt(),
            BuiltinId::Floor => x.floor(),
            BuiltinId::Ceil => x.ceil(),
            _ => unreachable!(),
        };
        Ok(RVal::Float(res))
    }
}
