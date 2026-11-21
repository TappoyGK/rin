//! リスト補助、同値・順序比較、算術・数学演算を実装する。

use crate::bytecode::BuiltinId;
use crate::error::{Error, Span};
use crate::pool::{Obj, RVal};

use super::Runner;

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

    pub(super) fn list_get(&self, mut v: RVal, mut idx: usize) -> Option<RVal> {
        while let RVal::Obj(id) = v {
            let Obj::Cons { head, tail, .. } = self.pool.get(id) else {
                break;
            };
            if idx == 0 {
                return Some(*head);
            }
            idx -= 1;
            v = *tail;
        }
        None
    }

    pub(super) fn list_to_vec(&self, mut v: RVal) -> Vec<RVal> {
        let mut out = Vec::with_capacity(self.pool.list_len(v));
        while let RVal::Obj(id) = v {
            let Obj::Cons { head, tail, .. } = self.pool.get(id) else {
                break;
            };
            out.push(*head);
            v = *tail;
        }
        out
    }

    pub(super) fn build_list_from_owned(
        &mut self,
        items: Vec<RVal>,
        span: Span,
    ) -> Result<RVal, Error> {
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
            return Err(Error::runtime(span, format!("'{op}' takes 1 argument")));
        };
        self.expect_list(op, *v, span)?;
        Ok(*v)
    }

    pub(super) fn expect_int(&self, op: &str, v: RVal, span: Span) -> Result<i64, Error> {
        match v {
            RVal::Int(n) => Ok(n),
            other => Err(Error::runtime(
                span,
                format!("'{op}' expects i64, got {}", other.type_name(self.pool)),
            )),
        }
    }

    pub(super) fn values_equal(&self, a: RVal, b: RVal) -> bool {
        let mut work = crate::work::Work::<_, 8>::new();
        let mut next = Some((a, b));
        while let Some((a, b)) = next.take().or_else(|| work.pop()) {
            let equal = match (a, b) {
                (RVal::Void, RVal::Void) => true,
                (RVal::Int(x), RVal::Int(y)) => x == y,
                (RVal::Float(x), RVal::Float(y)) => x == y,
                (RVal::Bool(x), RVal::Bool(y)) => x == y,
                (RVal::Char(x), RVal::Char(y)) => x == y,
                (RVal::None, RVal::None) => true,
                (RVal::EmptyList, RVal::EmptyList) => true,
                (RVal::Builtin(x), RVal::Builtin(y)) => x == y,
                (RVal::Obj(id1), RVal::Obj(id2)) => {
                    if id1 == id2 {
                        continue;
                    }
                    match (self.pool.get(id1), self.pool.get(id2)) {
                        (
                            Obj::Cons {
                                head: h1,
                                tail: t1,
                                len: l1,
                            },
                            Obj::Cons {
                                head: h2,
                                tail: t2,
                                len: l2,
                            },
                        ) => {
                            if l1 != l2 {
                                return false;
                            }
                            work.push((*t1, *t2));
                            next = Some((*h1, *h2));
                            true
                        }
                        (Obj::Str(s1), Obj::Str(s2)) => s1 == s2,
                        (Obj::Array(xs1), Obj::Array(xs2)) | (Obj::Tuple(xs1), Obj::Tuple(xs2)) => {
                            if xs1.len() != xs2.len() {
                                return false;
                            }
                            work.extend(xs1.iter().zip(xs2.iter()).rev().map(|(x, y)| (*x, *y)));
                            true
                        }
                        (
                            Obj::Struct {
                                name: n1,
                                fields: f1,
                            },
                            Obj::Struct {
                                name: n2,
                                fields: f2,
                            },
                        ) => {
                            if n1 != n2 || f1.len() != f2.len() {
                                return false;
                            }
                            for ((k1, v1), (k2, v2)) in f1.iter().zip(f2.iter()).rev() {
                                if k1 != k2 {
                                    return false;
                                }
                                work.push((*v1, *v2));
                            }
                            true
                        }
                        (
                            Obj::Variant {
                                type_name: t1,
                                variant: v1,
                                data: d1,
                            },
                            Obj::Variant {
                                type_name: t2,
                                variant: v2,
                                data: d2,
                            },
                        ) => {
                            t1 == t2
                                && v1 == v2
                                && match (d1, d2) {
                                    (None, None) => true,
                                    (Some(x), Some(y)) => {
                                        work.push((*x, *y));
                                        true
                                    }
                                    _ => false,
                                }
                        }
                        (Obj::Some(x), Obj::Some(y))
                        | (Obj::Ok(x), Obj::Ok(y))
                        | (Obj::Err(x), Obj::Err(y)) => {
                            work.push((*x, *y));
                            true
                        }
                        _ => false,
                    }
                }
                _ => false,
            };
            if !equal {
                return false;
            }
        }
        true
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
                .ok_or_else(|| Error::runtime(span, "cannot order NaN values")),
            (RVal::Char(x), RVal::Char(y)) => Ok(x.cmp(&y)),
            (RVal::Obj(id1), RVal::Obj(id2)) => match (self.pool.get(id1), self.pool.get(id2)) {
                (Obj::Str(s1), Obj::Str(s2)) => Ok(s1.cmp(s2)),
                _ => Err(Error::runtime(
                    span,
                    format!(
                        "cannot order {} and {}",
                        a.type_name(self.pool),
                        b.type_name(self.pool)
                    ),
                )),
            },
            _ => Err(Error::runtime(
                span,
                format!(
                    "cannot order {} and {}",
                    a.type_name(self.pool),
                    b.type_name(self.pool)
                ),
            )),
        }
    }

    pub(super) fn arith(&self, op: BuiltinId, args: &[RVal], span: Span) -> Result<RVal, Error> {
        if op == BuiltinId::Sub && args.len() == 1 {
            return match args[0] {
                RVal::Int(a) => a
                    .checked_neg()
                    .map(RVal::Int)
                    .ok_or_else(|| Error::runtime(span, "i64 overflow")),
                RVal::Float(a) => Ok(RVal::Float(-a)),
                other => Err(Error::runtime(
                    span,
                    format!("cannot negate {}", other.type_name(self.pool)),
                )),
            };
        }
        if (op == BuiltinId::Add || op == BuiltinId::Mul) && args.len() >= 2 {
            let mut acc = args[0];
            for &rhs in &args[1..] {
                acc = self.arith_bin(op, acc, rhs, span)?;
            }
            return Ok(acc);
        }
        let [a, b] = args else {
            return Err(Error::runtime(
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
                            return Err(Error::runtime(span, "division by zero"));
                        }
                        x.checked_div(y)
                    }
                    BuiltinId::Rem => {
                        if y == 0 {
                            return Err(Error::runtime(span, "division by zero"));
                        }
                        x.checked_rem(y)
                    }
                    _ => unreachable!(),
                };
                res.map(RVal::Int)
                    .ok_or_else(|| Error::runtime(span, "i64 overflow"))
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

    pub(super) fn eq_cmp(&self, want_eq: bool, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [a, b] = args else {
            return Err(Error::runtime(span, "comparison takes 2 arguments"));
        };
        if a.type_name(self.pool) != b.type_name(self.pool) {
            return Err(Error::runtime(
                span,
                format!(
                    "cannot compare {} and {}",
                    a.type_name(self.pool),
                    b.type_name(self.pool)
                ),
            ));
        }
        let eq = self.values_equal(*a, *b);
        Ok(RVal::Bool(if want_eq { eq } else { !eq }))
    }

    pub(super) fn ord_cmp(&self, op: BuiltinId, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [a, b] = args else {
            return Err(Error::runtime(
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
                span,
                format!("'{}' takes 1 argument", b.name()),
            ));
        };
        if b == BuiltinId::Abs {
            return match v {
                RVal::Int(n) => n
                    .checked_abs()
                    .map(RVal::Int)
                    .ok_or_else(|| Error::runtime(span, "i64 overflow")),
                RVal::Float(x) => Ok(RVal::Float(x.abs())),
                other => Err(Error::runtime(
                    span,
                    format!("'abs' expects number, got {}", other.type_name(self.pool)),
                )),
            };
        }
        let RVal::Float(x) = v else {
            return Err(Error::runtime(
                span,
                format!("'{}' expects f64, got {}", b.name(), v.type_name(self.pool)),
            ));
        };
        let res = match b {
            BuiltinId::Sin => x.sin(),
            BuiltinId::Cos => x.cos(),
            BuiltinId::Sqrt => x.sqrt(),
            BuiltinId::Floor => x.floor(),
            BuiltinId::Ceil => x.ceil(),
            _ => unreachable!(),
        };
        Ok(RVal::Float(res))
    }
}
