//! 値の比較と、比較対象の走査・文字列長に応じたステップ計上。
use super::*;
use crate::eval::runner::try_charge_steps;

impl Runner<'_, '_> {
    // None は比較途中でステップ残量を使い切ったことを表す。
    #[inline(always)]
    pub(in crate::eval) fn values_equal(&mut self, a: RVal, b: RVal) -> Option<bool> {
        if let (RVal::Obj(aid), RVal::Obj(bid)) = (a, b)
            && aid != bid
        {
            return self.objects_equal(a, b);
        }
        if !self.try_charge(1) {
            return None;
        }
        Some(Self::scalars_equal(a, b))
    }

    // オブジェクトの中身には触れない。呼び出し側で比較回数を計上する。
    #[inline(always)]
    pub(in crate::eval) fn scalars_equal(a: RVal, b: RVal) -> bool {
        match (a, b) {
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
        }
    }

    #[inline(always)]
    fn objects_equal(&mut self, a: RVal, b: RVal) -> Option<bool> {
        objects_equal(self.pool, &mut self.steps_left, a, b)
    }
}

#[inline(never)]
fn objects_equal(
    pool: &crate::pool::Pool<'_>,
    remaining: &mut usize,
    a: RVal,
    b: RVal,
) -> Option<bool> {
    let mut work = crate::work::Work::<_, 8>::new();
    let mut next = Some((a, b));
    while let Some((a, b)) = next.take().or_else(|| work.pop()) {
        if !try_charge_steps(remaining, 1) {
            return None;
        }
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
                match (pool.get(id1), pool.get(id2)) {
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
                            return Some(false);
                        }
                        if matches!((*h1, *h2), (RVal::Obj(a), RVal::Obj(b)) if a != b) {
                            work.push((*t1, *t2));
                            next = Some((*h1, *h2));
                        } else {
                            // スカラーの先頭はその場で比較し、末尾を作業スタックへ
                            // 積んですぐ取り出す往復を省く。計上順は先頭→末尾のまま。
                            if !try_charge_steps(remaining, 1) {
                                return None;
                            }
                            if !Runner::scalars_equal(*h1, *h2) {
                                return Some(false);
                            }
                            next = Some((*t1, *t2));
                        }
                        true
                    }
                    (Obj::Str(s1), Obj::Str(s2)) => {
                        if !try_charge_steps(remaining, s1.len().min(s2.len())) {
                            return None;
                        }
                        s1 == s2
                    }
                    (Obj::Array(xs1), Obj::Array(xs2)) | (Obj::Tuple(xs1), Obj::Tuple(xs2)) => {
                        if xs1.len() != xs2.len() {
                            return Some(false);
                        }
                        if !try_charge_steps(remaining, xs1.len()) {
                            return None;
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
                        if !try_charge_steps(remaining, n1.len().min(n2.len())) {
                            return None;
                        }
                        if n1 != n2 || f1.len() != f2.len() {
                            return Some(false);
                        }
                        match scalar_fields_equal(remaining, f1, f2) {
                            ScalarFields::Equal => continue,
                            ScalarFields::Different => return Some(false),
                            ScalarFields::OutOfSteps => return None,
                            ScalarFields::NeedsObjects => {}
                        }
                        for ((k1, v1), (k2, v2)) in f1.iter().zip(f2.iter()).rev() {
                            if !try_charge_steps(remaining, 1 + k1.len().min(k2.len())) {
                                return None;
                            }
                            if k1 != k2 {
                                return Some(false);
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
                        if !try_charge_steps(remaining, t1.len().min(t2.len())) {
                            return None;
                        }
                        if !try_charge_steps(remaining, v1.len().min(v2.len())) {
                            return None;
                        }
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
            return Some(false);
        }
    }
    Some(true)
}

// スカラーだけの構造体は作業スタックへの出し入れを省く。
// キーの検査に使える残量を守り、値の計上は全キーの検査後に行う。
// 中身を見る必要のあるオブジェクトを見つけたら、残量を変更せず通常の比較へ戻す。
enum ScalarFields {
    Equal,
    Different,
    OutOfSteps,
    NeedsObjects,
}

#[inline(never)]
fn scalar_fields_equal(
    remaining: &mut usize,
    a: &std::collections::BTreeMap<std::rc::Rc<str>, RVal>,
    b: &std::collections::BTreeMap<std::rc::Rc<str>, RVal>,
) -> ScalarFields {
    let mut left = *remaining;
    let mut mismatch = None;
    for (index, ((ka, va), (kb, vb))) in a.iter().zip(b.iter()).enumerate().rev() {
        if !try_charge_steps(&mut left, 1 + ka.len().min(kb.len())) {
            return ScalarFields::OutOfSteps;
        }
        if ka != kb {
            *remaining = left;
            return ScalarFields::Different;
        }
        if matches!((*va, *vb), (RVal::Obj(x), RVal::Obj(y)) if x != y) {
            return ScalarFields::NeedsObjects;
        }
        if !Runner::scalars_equal(*va, *vb) {
            mismatch = Some(index + 1);
        }
    }
    if !try_charge_steps(&mut left, mismatch.unwrap_or(a.len())) {
        return ScalarFields::OutOfSteps;
    }
    *remaining = left;
    if mismatch.is_some() {
        ScalarFields::Different
    } else {
        ScalarFields::Equal
    }
}
