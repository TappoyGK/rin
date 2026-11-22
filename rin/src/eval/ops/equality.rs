//! 値の比較と、比較対象の走査・文字列長に応じたステップ計上。
use super::*;
use crate::eval::runner::charge_steps;

impl Runner<'_, '_> {
    #[inline]
    pub(in crate::eval) fn values_equal(
        &mut self,
        a: RVal,
        b: RVal,
        span: Span,
    ) -> Result<bool, Error> {
        if let (RVal::Obj(aid), RVal::Obj(bid)) = (a, b)
            && aid != bid
        {
            return self.objects_equal(a, b, span);
        }
        self.charge(1, span)?;
        Ok(match (a, b) {
            (RVal::Void, RVal::Void)
            | (RVal::None, RVal::None)
            | (RVal::EmptyList, RVal::EmptyList) => true,
            (RVal::Int(a), RVal::Int(b)) => a == b,
            (RVal::Float(a), RVal::Float(b)) => a == b,
            (RVal::Bool(a), RVal::Bool(b)) => a == b,
            (RVal::Char(a), RVal::Char(b)) => a == b,
            (RVal::Builtin(a), RVal::Builtin(b)) => a == b,
            (RVal::Obj(aid), RVal::Obj(bid)) => aid == bid,
            _ => false,
        })
    }

    #[inline(never)]
    fn objects_equal(&mut self, a: RVal, b: RVal, span: Span) -> Result<bool, Error> {
        let mut work = crate::work::Work::<_, 8>::new();
        let mut next = Some((a, b));
        while let Some((a, b)) = next.take().or_else(|| work.pop()) {
            self.charge(1, span)?;
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
                                return Ok(false);
                            }
                            work.push((*t1, *t2));
                            next = Some((*h1, *h2));
                            true
                        }
                        (Obj::Str(s1), Obj::Str(s2)) => {
                            charge_steps(
                                &mut self.steps,
                                self.limits.steps,
                                s1.len().min(s2.len()),
                                span,
                            )?;
                            s1 == s2
                        }
                        (Obj::Array(xs1), Obj::Array(xs2)) | (Obj::Tuple(xs1), Obj::Tuple(xs2)) => {
                            if xs1.len() != xs2.len() {
                                return Ok(false);
                            }
                            charge_steps(&mut self.steps, self.limits.steps, xs1.len(), span)?;
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
                            charge_steps(
                                &mut self.steps,
                                self.limits.steps,
                                n1.len().min(n2.len()),
                                span,
                            )?;
                            if n1 != n2 || f1.len() != f2.len() {
                                return Ok(false);
                            }
                            for ((k1, v1), (k2, v2)) in f1.iter().zip(f2.iter()).rev() {
                                charge_steps(&mut self.steps, self.limits.steps, 1, span)?;
                                charge_steps(
                                    &mut self.steps,
                                    self.limits.steps,
                                    k1.len().min(k2.len()),
                                    span,
                                )?;
                                if k1 != k2 {
                                    return Ok(false);
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
                            charge_steps(
                                &mut self.steps,
                                self.limits.steps,
                                t1.len().min(t2.len()),
                                span,
                            )?;
                            charge_steps(
                                &mut self.steps,
                                self.limits.steps,
                                v1.len().min(v2.len()),
                                span,
                            )?;
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
                return Ok(false);
            }
        }
        Ok(true)
    }
}
