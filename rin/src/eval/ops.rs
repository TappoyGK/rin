//! リスト補助、同値・順序比較、算術・数学演算を実装する。

use crate::bytecode::BuiltinId;
use crate::error::{Error, Span};
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
    ) -> Result<Option<RVal>, Error> {
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

    // None はステップ残量不足。Error の生成は呼び出し側の失敗経路だけで行う。
    pub(super) fn list_to_vec(&mut self, mut v: RVal) -> Option<Vec<RVal>> {
        if !self.try_charge(self.pool.list_len(v)) {
            return None;
        }
        let mut out = Vec::with_capacity(self.pool.list_len(v));
        while let RVal::Obj(id) = v {
            let Obj::Cons { head, tail, .. } = self.pool.get(id) else {
                break;
            };
            out.push(*head);
            v = *tail;
        }
        Some(out)
    }

    pub(super) fn build_list_from_owned(
        &mut self,
        items: Vec<RVal>,
        span: Span,
    ) -> Result<RVal, Error> {
        if !self.try_charge(items.len()) {
            self.release_all(items);
            return self.step_limit_result(span);
        }
        let mut acc = RVal::EmptyList;
        for (idx, item) in items.iter().enumerate().rev() {
            match self.pool.alloc_cons(*item, acc, span) {
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
            self.pool.release_val(*item);
        }
        Ok(acc)
    }

    pub(super) fn build_list_from_owned_slice(
        &mut self,
        start: usize,
        span: Span,
    ) -> Result<RVal, Error> {
        let mut acc = RVal::EmptyList;
        let end = self.stack.len();
        if !self.try_charge(end - start) {
            self.release_stack_from(start);
            return self.step_limit_result(span);
        }
        for idx in (start..end).rev() {
            let item = self.stack[idx];
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
    ) -> Result<RVal, Error> {
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

    pub(super) fn expect_list(&self, op: &str, v: RVal, span: Span) -> Result<(), Error> {
        if self.is_list(v) {
            Ok(())
        } else {
            Err(Error::runtime(
                "fn.list",
                span,
                format!("'{op}' expects list, got {}", v.type_name(self.pool)),
            ))
        }
    }

    pub(super) fn expect_one_list(
        &self,
        op: &str,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, Error> {
        let [v] = args else {
            return Err(Error::runtime(
                "fn.list",
                span,
                format!("'{op}' takes 1 argument"),
            ));
        };
        self.expect_list(op, *v, span)?;
        Ok(*v)
    }

    pub(super) fn expect_int(&self, op: &str, v: RVal, span: Span) -> Result<i64, Error> {
        match v {
            RVal::Int(n) => Ok(n),
            other => Err(Error::runtime(
                "fn.list",
                span,
                format!("'{op}' expects i64, got {}", other.type_name(self.pool)),
            )),
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
                    Obj::Str(s) => s.len(),
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

    pub(super) fn compare_vals(
        &self,
        a: RVal,
        b: RVal,
        span: Span,
    ) -> Result<std::cmp::Ordering, Error> {
        match (a, b) {
            (RVal::Int(x), RVal::Int(y)) => Ok(x.cmp(&y)),
            (RVal::Float(x), RVal::Float(y)) => x
                .partial_cmp(&y)
                .ok_or_else(|| Error::runtime("fn.list", span, "cannot order NaN values")),
            (RVal::Char(x), RVal::Char(y)) => Ok(x.cmp(&y)),
            (RVal::Obj(id1), RVal::Obj(id2)) => match (self.pool.get(id1), self.pool.get(id2)) {
                (Obj::Str(s1), Obj::Str(s2)) => Ok(s1.cmp(s2)),
                _ => Err(Error::runtime(
                    "fn.list",
                    span,
                    format!(
                        "cannot order {} and {}",
                        a.type_name(self.pool),
                        b.type_name(self.pool)
                    ),
                )),
            },
            _ => Err(Error::runtime(
                "fn.list",
                span,
                format!(
                    "cannot order {} and {}",
                    a.type_name(self.pool),
                    b.type_name(self.pool)
                ),
            )),
        }
    }

    pub(super) fn arith(
        &mut self,
        op: BuiltinId,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, Error> {
        if op == BuiltinId::Sub && args.len() == 1 {
            return match args[0] {
                RVal::Int(a) => a
                    .checked_neg()
                    .map(RVal::Int)
                    .ok_or_else(|| Error::runtime("op.arith", span, "i64 overflow")),
                RVal::Float(a) => Ok(RVal::Float(-a)),
                other => Err(Error::runtime(
                    "op.arith",
                    span,
                    format!("cannot negate {}", other.type_name(self.pool)),
                )),
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
            return Err(Error::runtime(
                "op.arith",
                span,
                format!("'{}' takes 2 arguments", op.name()),
            ));
        };
        self.arith_bin(op, *a, *b, span)
    }

    pub(super) fn arith_bin(
        &self,
        op: BuiltinId,
        a: RVal,
        b: RVal,
        span: Span,
    ) -> Result<RVal, Error> {
        match (a, b) {
            (RVal::Int(x), RVal::Int(y)) => {
                let res = match op {
                    BuiltinId::Add => x.checked_add(y),
                    BuiltinId::Sub => x.checked_sub(y),
                    BuiltinId::Mul => x.checked_mul(y),
                    BuiltinId::Div => {
                        if y == 0 {
                            return Err(Error::runtime("op.arith", span, "division by zero"));
                        }
                        x.checked_div(y)
                    }
                    BuiltinId::Rem => {
                        if y == 0 {
                            return Err(Error::runtime("op.arith", span, "division by zero"));
                        }
                        x.checked_rem(y)
                    }
                    _ => unreachable!(),
                };
                res.map(RVal::Int)
                    .ok_or_else(|| Error::runtime("op.arith", span, "i64 overflow"))
            }
            (RVal::Float(x), RVal::Float(y)) => match op {
                BuiltinId::Add => Ok(RVal::Float(x + y)),
                BuiltinId::Sub => Ok(RVal::Float(x - y)),
                BuiltinId::Mul => Ok(RVal::Float(x * y)),
                BuiltinId::Div => Ok(RVal::Float(x / y)),
                BuiltinId::Rem => Ok(RVal::Float(x % y)),
                _ => unreachable!(),
            },
            _ => Err(Error::runtime(
                "op.arith",
                span,
                format!(
                    "cannot apply '{}' to {} and {} (no implicit conversion)",
                    op.name(),
                    a.type_name(self.pool),
                    b.type_name(self.pool)
                ),
            )),
        }
    }

    pub(super) fn eq_cmp(
        &mut self,
        want_eq: bool,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, Error> {
        let [a, b] = args else {
            return Err(Error::runtime(
                "op.cmp",
                span,
                "comparison takes 2 arguments",
            ));
        };
        if a.type_name(self.pool) != b.type_name(self.pool) {
            return Err(Error::runtime(
                "op.cmp",
                span,
                format!(
                    "cannot compare {} and {}",
                    a.type_name(self.pool),
                    b.type_name(self.pool)
                ),
            ));
        }
        let Some(eq) = self.values_equal(*a, *b) else {
            return self.step_limit_result(span);
        };
        Ok(RVal::Bool(if want_eq { eq } else { !eq }))
    }

    pub(super) fn ord_cmp(&self, op: BuiltinId, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [a, b] = args else {
            return Err(Error::runtime(
                "op.cmp",
                span,
                format!("'{}' takes 2 arguments", op.name()),
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
            _ => {
                return Err(Error::runtime(
                    "op.cmp",
                    span,
                    format!(
                        "cannot compare {} and {}",
                        a.type_name(self.pool),
                        b.type_name(self.pool)
                    ),
                ));
            }
        };
        Ok(RVal::Bool(res))
    }

    pub(super) fn math(&self, b: BuiltinId, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [v] = args else {
            return Err(Error::runtime(
                "fn.math",
                span,
                format!("'{}' takes 1 argument", b.name()),
            ));
        };
        if b == BuiltinId::Abs {
            return match v {
                RVal::Int(n) => n
                    .checked_abs()
                    .map(RVal::Int)
                    .ok_or_else(|| Error::runtime("fn.math", span, "i64 overflow")),
                RVal::Float(x) => Ok(RVal::Float(x.abs())),
                other => Err(Error::runtime(
                    "fn.math",
                    span,
                    format!("'abs' expects number, got {}", other.type_name(self.pool)),
                )),
            };
        }
        let RVal::Float(x) = v else {
            return Err(Error::runtime(
                "fn.math",
                span,
                format!("'{}' expects f64, got {}", b.name(), v.type_name(self.pool)),
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
