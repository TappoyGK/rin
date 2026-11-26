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
        let mark = self.pool.work.mark(&self.pool.core);
        let res = objects_equal(self.pool, &mut self.steps_left, a, b);
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }
}

struct EqWorkStack {
    inline: [(RVal, RVal); 8],
    inline_len: usize,
    overflow: crate::mem::RawVec<(RVal, RVal)>,
}

impl EqWorkStack {
    fn new() -> Self {
        Self {
            inline: [(RVal::Void, RVal::Void); 8],
            inline_len: 0,
            overflow: crate::mem::RawVec::new(),
        }
    }

    #[inline(always)]
    fn push(&mut self, pool: &mut crate::pool::Pool<'_>, pair: (RVal, RVal)) -> bool {
        if self.overflow.is_empty() && self.inline_len < self.inline.len() {
            self.inline[self.inline_len] = pair;
            self.inline_len += 1;
            true
        } else {
            self.overflow
                .push_work(&mut pool.work, &mut pool.core, pair)
                .is_some()
        }
    }

    #[inline(always)]
    fn pop(&mut self) -> Option<(RVal, RVal)> {
        if let Some(v) = self.overflow.pop() {
            Some(v)
        } else if self.inline_len > 0 {
            self.inline_len -= 1;
            Some(self.inline[self.inline_len])
        } else {
            None
        }
    }
}

#[inline(never)]
fn objects_equal(
    pool: &mut crate::pool::Pool<'_>,
    remaining: &mut usize,
    a: RVal,
    b: RVal,
) -> Option<bool> {
    let mut work = EqWorkStack::new();
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
                enum EqStep {
                    Simple(bool),
                    Cons(RVal, RVal, RVal, RVal),
                    Slice(crate::pool::ObjSlice<RVal>, crate::pool::ObjSlice<RVal>),
                    Struct(
                        crate::pool::StructDescRef,
                        crate::pool::ObjSlice<RVal>,
                        crate::pool::StructDescRef,
                        crate::pool::ObjSlice<RVal>,
                    ),
                    Pair(RVal, RVal),
                }
                let step = match (pool.get(id1), pool.get(id2)) {
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
                        EqStep::Cons(*h1, *t1, *h2, *t2)
                    }
                    (Obj::Str(s1), Obj::Str(s2)) => {
                        if !try_charge_steps(remaining, s1.len().min(s2.len())) {
                            return None;
                        }
                        EqStep::Simple(s1.as_str() == s2.as_str())
                    }
                    (Obj::Array(xs1), Obj::Array(xs2)) | (Obj::Tuple(xs1), Obj::Tuple(xs2)) => {
                        if xs1.len() != xs2.len() {
                            return Some(false);
                        }
                        if !try_charge_steps(remaining, xs1.len()) {
                            return None;
                        }
                        EqStep::Slice(*xs1, *xs2)
                    }
                    (
                        Obj::Struct {
                            desc: d1,
                            values: v1,
                        },
                        Obj::Struct {
                            desc: d2,
                            values: v2,
                        },
                    ) => EqStep::Struct(*d1, *v1, *d2, *v2),
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
                        if t1.as_str() != t2.as_str() || v1.as_str() != v2.as_str() {
                            EqStep::Simple(false)
                        } else {
                            match (d1, d2) {
                                (None, None) => EqStep::Simple(true),
                                (Some(x), Some(y)) => EqStep::Pair(*x, *y),
                                _ => EqStep::Simple(false),
                            }
                        }
                    }
                    (Obj::Some(x), Obj::Some(y))
                    | (Obj::Ok(x), Obj::Ok(y))
                    | (Obj::Err(x), Obj::Err(y)) => EqStep::Pair(*x, *y),
                    _ => EqStep::Simple(false),
                };
                match step {
                    EqStep::Simple(eq) => eq,
                    EqStep::Pair(x, y) => {
                        if !work.push(pool, (x, y)) {
                            return None;
                        }
                        true
                    }
                    EqStep::Cons(h1, t1, h2, t2) => {
                        if matches!((h1, h2), (RVal::Obj(a), RVal::Obj(b)) if a != b) {
                            if !work.push(pool, (t1, t2)) {
                                return None;
                            }
                            next = Some((h1, h2));
                        } else {
                            // スカラーの先頭はその場で比較し、末尾を作業スタックへ
                            // 積んですぐ取り出す往復を省く。計上順は先頭→末尾のまま。
                            if !try_charge_steps(remaining, 1) {
                                return None;
                            }
                            if !Runner::scalars_equal(h1, h2) {
                                return Some(false);
                            }
                            next = Some((t1, t2));
                        }
                        true
                    }
                    EqStep::Slice(xs1, xs2) => {
                        for (x, y) in xs1.as_slice().iter().zip(xs2.as_slice().iter()).rev() {
                            if !work.push(pool, (*x, *y)) {
                                return None;
                            }
                        }
                        true
                    }
                    EqStep::Struct(d1, v1, d2, v2) => {
                        let n1 = pool.struct_desc_name(d1);
                        let n2 = pool.struct_desc_name(d2);
                        if !try_charge_steps(remaining, n1.len().min(n2.len())) {
                            return None;
                        }
                        if n1 != n2 || v1.len() != v2.len() {
                            return Some(false);
                        }
                        match scalar_fields_equal(
                            pool,
                            remaining,
                            d1,
                            v1.as_slice(),
                            d2,
                            v2.as_slice(),
                        ) {
                            ScalarFields::Equal => continue,
                            ScalarFields::Different => return Some(false),
                            ScalarFields::OutOfSteps => return None,
                            ScalarFields::NeedsObjects => {}
                        }
                        let len = v1.len();
                        for idx in (0..len).rev() {
                            let k1 = pool.struct_desc_field_name(d1, idx);
                            let k2 = pool.struct_desc_field_name(d2, idx);
                            if !try_charge_steps(remaining, 1 + k1.len().min(k2.len())) {
                                return None;
                            }
                            if k1 != k2 {
                                return Some(false);
                            }
                            let pair = (v1.as_slice()[idx], v2.as_slice()[idx]);
                            if !work.push(pool, pair) {
                                return None;
                            }
                        }
                        true
                    }
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
    pool: &crate::pool::Pool<'_>,
    remaining: &mut usize,
    d1: crate::pool::StructDescRef,
    v1: &[RVal],
    d2: crate::pool::StructDescRef,
    v2: &[RVal],
) -> ScalarFields {
    let mut left = *remaining;
    let mut mismatch = None;
    for index in (0..v1.len()).rev() {
        let ka = pool.struct_desc_field_name(d1, index);
        let kb = pool.struct_desc_field_name(d2, index);
        if !try_charge_steps(&mut left, 1 + ka.len().min(kb.len())) {
            return ScalarFields::OutOfSteps;
        }
        if ka != kb {
            *remaining = left;
            return ScalarFields::Different;
        }
        let va = v1[index];
        let vb = v2[index];
        if matches!((va, vb), (RVal::Obj(x), RVal::Obj(y)) if x != y) {
            return ScalarFields::NeedsObjects;
        }
        if !Runner::scalars_equal(va, vb) {
            mismatch = Some(index + 1);
        }
    }
    if !try_charge_steps(&mut left, mismatch.unwrap_or(v1.len())) {
        return ScalarFields::OutOfSteps;
    }
    *remaining = left;
    if mismatch.is_some() {
        ScalarFields::Different
    } else {
        ScalarFields::Equal
    }
}
