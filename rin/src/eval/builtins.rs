//! 組み込み関数および高階リスト・配列演算を実装する。

use crate::bytecode::BuiltinId;
use crate::error::{Error, Span};
use crate::pool::{Obj, RVal};

use super::{CallOrigin, Runner};

impl Runner<'_, '_> {
    pub(super) fn apply_builtin(
        &mut self,
        b: BuiltinId,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, Error> {
        match b {
            BuiltinId::Add | BuiltinId::Sub | BuiltinId::Mul | BuiltinId::Div | BuiltinId::Rem => {
                self.arith(b, args, span)
            }
            BuiltinId::Eq | BuiltinId::Ne => self.eq_cmp(b == BuiltinId::Eq, args, span),
            BuiltinId::Lt | BuiltinId::Le | BuiltinId::Gt | BuiltinId::Ge => {
                self.ord_cmp(b, args, span)
            }
            BuiltinId::And => {
                if args.is_empty() {
                    return Err(Error::runtime(
                        "op.logic",
                        span,
                        "'and' requires at least 1 argument",
                    ));
                }
                for v in args {
                    match v {
                        RVal::Bool(true) => {}
                        RVal::Bool(false) => return Ok(RVal::Bool(false)),
                        other => {
                            return Err(Error::runtime(
                                "op.logic",
                                span,
                                format!("expected bool, got {}", other.type_name(self.pool)),
                            ));
                        }
                    }
                }
                Ok(RVal::Bool(true))
            }
            BuiltinId::Or => {
                if args.is_empty() {
                    return Err(Error::runtime(
                        "op.logic",
                        span,
                        "'or' requires at least 1 argument",
                    ));
                }
                for v in args {
                    match v {
                        RVal::Bool(true) => return Ok(RVal::Bool(true)),
                        RVal::Bool(false) => {}
                        other => {
                            return Err(Error::runtime(
                                "op.logic",
                                span,
                                format!("expected bool, got {}", other.type_name(self.pool)),
                            ));
                        }
                    }
                }
                Ok(RVal::Bool(false))
            }
            BuiltinId::Not => {
                let [v] = args else {
                    return Err(Error::runtime("op.logic", span, "'not' takes 1 argument"));
                };
                match v {
                    RVal::Bool(b) => Ok(RVal::Bool(!b)),
                    other => Err(Error::runtime(
                        "op.logic",
                        span,
                        format!("'not' expects bool, got {}", other.type_name(self.pool)),
                    )),
                }
            }
            BuiltinId::AsF64 => {
                let [v] = args else {
                    return Err(Error::runtime(
                        "fn.convert",
                        span,
                        "'as_f64' takes 1 argument",
                    ));
                };
                match v {
                    RVal::Int(n) => Ok(RVal::Float(*n as f64)),
                    RVal::Float(x) => Ok(RVal::Float(*x)),
                    other => Err(Error::runtime(
                        "fn.convert",
                        span,
                        format!(
                            "'as_f64' expects number, got {}",
                            other.type_name(self.pool)
                        ),
                    )),
                }
            }
            BuiltinId::AsI64 => {
                let [v] = args else {
                    return Err(Error::runtime(
                        "fn.convert",
                        span,
                        "'as_i64' takes 1 argument",
                    ));
                };
                match v {
                    RVal::Int(n) => Ok(RVal::Int(*n)),
                    RVal::Float(x) => {
                        if !x.is_finite() || *x < (i64::MIN as f64) || *x > (i64::MAX as f64) {
                            return Err(Error::runtime(
                                "fn.convert",
                                span,
                                "float out of i64 range",
                            ));
                        }
                        Ok(RVal::Int(*x as i64))
                    }
                    other => Err(Error::runtime(
                        "fn.convert",
                        span,
                        format!(
                            "'as_i64' expects number, got {}",
                            other.type_name(self.pool)
                        ),
                    )),
                }
            }
            BuiltinId::Sin
            | BuiltinId::Cos
            | BuiltinId::Sqrt
            | BuiltinId::Abs
            | BuiltinId::Floor
            | BuiltinId::Ceil => self.math(b, args, span),
            BuiltinId::List => self.build_list_from_borrowed(args, span),
            BuiltinId::Len => {
                let [v] = args else {
                    return Err(Error::runtime("fn.list", span, "'len' takes 1 argument"));
                };
                if self.is_list(*v) {
                    return Ok(RVal::Int(self.pool.list_len(*v) as i64));
                }
                if let RVal::Obj(id) = v {
                    match self.pool.get(*id) {
                        Obj::Array(arr) => return Ok(RVal::Int(arr.len() as i64)),
                        Obj::Str(s) => return Ok(RVal::Int(s.chars().count() as i64)),
                        _ => {}
                    }
                }
                Err(Error::runtime(
                    "fn.list",
                    span,
                    format!(
                        "'len' expects list, array, or string, got {}",
                        v.type_name(self.pool)
                    ),
                ))
            }
            BuiltinId::IsEmpty => {
                let xs = self.expect_one_list("is_empty", args, span)?;
                Ok(RVal::Bool(self.pool.list_len(xs) == 0))
            }
            BuiltinId::Get => {
                let [list_v, idx_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'get' takes 2 arguments (seq, index)",
                    ));
                };
                let idx = self.expect_int("get", *idx_v, span)?;
                if let RVal::Obj(id) = *list_v
                    && let Obj::Array(arr) = self.pool.get(id)
                {
                    if idx < 0 || (idx as usize) >= arr.len() {
                        return Ok(RVal::None);
                    }
                    let item = arr[idx as usize];
                    self.pool.retain_val(item);
                    return Ok(RVal::Obj(self.pool.alloc_obj(Obj::Some(item), span)?));
                }
                self.expect_list("get", *list_v, span)?;
                if idx < 0 {
                    return Ok(RVal::None);
                }
                match self.list_get(*list_v, idx as usize) {
                    Some(item) => {
                        self.pool.retain_val(item);
                        Ok(RVal::Obj(self.pool.alloc_obj(Obj::Some(item), span)?))
                    }
                    None => Ok(RVal::None),
                }
            }
            BuiltinId::Car => {
                let xs = self.expect_one_list("car", args, span)?;
                match self.list_head(xs) {
                    Some(head) => {
                        self.pool.retain_val(head);
                        Ok(RVal::Obj(self.pool.alloc_obj(Obj::Some(head), span)?))
                    }
                    None => Ok(RVal::None),
                }
            }
            BuiltinId::Cdr => {
                let xs = self.expect_one_list("cdr", args, span)?;
                let tail = self.list_tail(xs);
                self.pool.retain_val(tail);
                Ok(tail)
            }
            BuiltinId::Last => {
                let xs = self.expect_one_list("last", args, span)?;
                let items = self.list_to_vec(xs);
                match items.last().copied() {
                    Some(v) => {
                        self.pool.retain_val(v);
                        Ok(RVal::Obj(self.pool.alloc_obj(Obj::Some(v), span)?))
                    }
                    None => Ok(RVal::None),
                }
            }
            BuiltinId::Cons => {
                let [item, list_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'cons' takes 2 arguments (item, list)",
                    ));
                };
                self.expect_list("cons", *list_v, span)?;
                self.pool.alloc_cons(*item, *list_v, span)
            }
            BuiltinId::Push => {
                let [list_v, item] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'push' takes 2 arguments (list, item)",
                    ));
                };
                self.expect_list("push", *list_v, span)?;
                let mut items = self.list_to_vec(*list_v);
                items.push(*item);
                self.build_list_from_borrowed(&items, span)
            }
            BuiltinId::Append => {
                let mut items = Vec::new();
                for &arg in args {
                    self.expect_list("append", arg, span)?;
                    items.extend(self.list_to_vec(arg));
                }
                self.build_list_from_borrowed(&items, span)
            }
            BuiltinId::Flatten => {
                let xs = self.expect_one_list("flatten", args, span)?;
                let outer = self.list_to_vec(xs);
                let mut items = Vec::new();
                for sub in outer {
                    self.expect_list("flatten", sub, span)?;
                    items.extend(self.list_to_vec(sub));
                }
                self.build_list_from_borrowed(&items, span)
            }
            BuiltinId::Slice => {
                let [list_v, start_v, end_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'slice' takes 3 arguments (list, start, end)",
                    ));
                };
                self.expect_list("slice", *list_v, span)?;
                let start = self.expect_int("slice", *start_v, span)?;
                let end = self.expect_int("slice", *end_v, span)?;
                let len = self.pool.list_len(*list_v) as i64;
                if start < 0 || end < start || end > len {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        format!("slice range {start}..{end} out of range (length {len})"),
                    ));
                }
                let items = self.list_to_vec(*list_v);
                self.build_list_from_borrowed(&items[start as usize..end as usize], span)
            }
            BuiltinId::Take => {
                let [list_v, n_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'take' takes 2 arguments (list, n)",
                    ));
                };
                self.expect_list("take", *list_v, span)?;
                let n = self.expect_int("take", *n_v, span)?;
                if n < 0 {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'take' count cannot be negative",
                    ));
                }
                let items = self.list_to_vec(*list_v);
                let end = (n as usize).min(items.len());
                self.build_list_from_borrowed(&items[..end], span)
            }
            BuiltinId::Drop => {
                let [list_v, n_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'drop' takes 2 arguments (list, n)",
                    ));
                };
                self.expect_list("drop", *list_v, span)?;
                let mut n = self.expect_int("drop", *n_v, span)?;
                if n < 0 {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'drop' count cannot be negative",
                    ));
                }
                let mut cur = *list_v;
                while n > 0 && self.pool.list_len(cur) > 0 {
                    cur = self.list_tail(cur);
                    n -= 1;
                }
                self.pool.retain_val(cur);
                Ok(cur)
            }
            BuiltinId::Reverse => {
                let xs = self.expect_one_list("reverse", args, span)?;
                let mut acc = RVal::EmptyList;
                let mut cur = xs;
                while let Some(head) = self.list_head(cur) {
                    match self.pool.alloc_cons(head, acc, span) {
                        Ok(next) => {
                            self.pool.release_val(acc);
                            acc = next;
                        }
                        Err(e) => {
                            self.pool.release_val(acc);
                            return Err(e);
                        }
                    }
                    cur = self.list_tail(cur);
                }
                Ok(acc)
            }
            BuiltinId::Range => {
                let [start_v, end_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'range' takes 2 arguments (start, end)",
                    ));
                };
                let start = self.expect_int("range", *start_v, span)?;
                let end = self.expect_int("range", *end_v, span)?;
                if end < start {
                    return Ok(RVal::EmptyList);
                }
                let mut acc = RVal::EmptyList;
                for n in (start..end).rev() {
                    self.steps += 1;
                    if self.steps > self.limits.steps {
                        self.pool.release_val(acc);
                        return Err(self.step_limit_err(span));
                    }
                    match self.pool.alloc_cons(RVal::Int(n), acc, span) {
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
            BuiltinId::Contains => {
                let [list_v, target] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'contains' takes 2 arguments (list, value)",
                    ));
                };
                self.expect_list("contains", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                for x in items {
                    if x.type_name(self.pool) != target.type_name(self.pool) {
                        return Err(Error::runtime(
                            "fn.list",
                            span,
                            format!(
                                "cannot compare {} and {}",
                                x.type_name(self.pool),
                                target.type_name(self.pool)
                            ),
                        ));
                    }
                    if self.values_equal(x, *target) {
                        return Ok(RVal::Bool(true));
                    }
                }
                Ok(RVal::Bool(false))
            }
            BuiltinId::Find => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'find' takes 2 arguments (fn, list)",
                    ));
                };
                self.expect_list("find", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                for x in items {
                    let res = self.apply(
                        *f,
                        &[x],
                        span,
                        CallOrigin::Builtin {
                            name: b.name(),
                            at: span,
                        },
                    )?;
                    match res {
                        RVal::Bool(true) => {
                            self.pool.retain_val(x);
                            return Ok(RVal::Obj(self.pool.alloc_obj(Obj::Some(x), span)?));
                        }
                        RVal::Bool(false) => {}
                        other => {
                            let tname = other.type_name(self.pool);
                            self.pool.release_val(res);
                            return Err(Error::runtime(
                                "fn.list",
                                span,
                                format!("'find' predicate must return bool, got {tname}"),
                            ));
                        }
                    }
                }
                Ok(RVal::None)
            }
            BuiltinId::Any => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'any' takes 2 arguments (fn, list)",
                    ));
                };
                self.expect_list("any", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                for x in items {
                    let res = self.apply(
                        *f,
                        &[x],
                        span,
                        CallOrigin::Builtin {
                            name: b.name(),
                            at: span,
                        },
                    )?;
                    match res {
                        RVal::Bool(true) => return Ok(RVal::Bool(true)),
                        RVal::Bool(false) => {}
                        other => {
                            let tname = other.type_name(self.pool);
                            self.pool.release_val(res);
                            return Err(Error::runtime(
                                "fn.list",
                                span,
                                format!("'any' predicate must return bool, got {tname}"),
                            ));
                        }
                    }
                }
                Ok(RVal::Bool(false))
            }
            BuiltinId::All => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'all' takes 2 arguments (fn, list)",
                    ));
                };
                self.expect_list("all", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                for x in items {
                    let res = self.apply(
                        *f,
                        &[x],
                        span,
                        CallOrigin::Builtin {
                            name: b.name(),
                            at: span,
                        },
                    )?;
                    match res {
                        RVal::Bool(true) => {}
                        RVal::Bool(false) => return Ok(RVal::Bool(false)),
                        other => {
                            let tname = other.type_name(self.pool);
                            self.pool.release_val(res);
                            return Err(Error::runtime(
                                "fn.list",
                                span,
                                format!("'all' predicate must return bool, got {tname}"),
                            ));
                        }
                    }
                }
                Ok(RVal::Bool(true))
            }
            BuiltinId::Map => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'map' takes 2 arguments (fn, list)",
                    ));
                };
                self.expect_list("map", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                let mut out = Vec::with_capacity(items.len());
                for x in items {
                    match self.apply(
                        *f,
                        &[x],
                        span,
                        CallOrigin::Builtin {
                            name: b.name(),
                            at: span,
                        },
                    ) {
                        Ok(v) => out.push(v),
                        Err(e) => {
                            self.release_all(out);
                            return Err(e);
                        }
                    }
                }
                self.build_list_from_owned(out, span)
            }
            BuiltinId::Filter => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'filter' takes 2 arguments (fn, list)",
                    ));
                };
                self.expect_list("filter", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                let mut out = Vec::new();
                for x in items {
                    let res = match self.apply(
                        *f,
                        &[x],
                        span,
                        CallOrigin::Builtin {
                            name: b.name(),
                            at: span,
                        },
                    ) {
                        Ok(v) => v,
                        Err(e) => {
                            self.release_all(out);
                            return Err(e);
                        }
                    };
                    match res {
                        RVal::Bool(true) => {
                            self.pool.retain_val(x);
                            out.push(x);
                        }
                        RVal::Bool(false) => {}
                        other => {
                            let tname = other.type_name(self.pool);
                            self.pool.release_val(res);
                            self.release_all(out);
                            return Err(Error::runtime(
                                "fn.list",
                                span,
                                format!("'filter' predicate must return bool, got {tname}"),
                            ));
                        }
                    }
                }
                self.build_list_from_owned(out, span)
            }
            BuiltinId::FlatMap => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'flat_map' takes 2 arguments (fn, list)",
                    ));
                };
                self.expect_list("flat_map", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                let mut out = Vec::new();
                for x in items {
                    let mapped = match self.apply(
                        *f,
                        &[x],
                        span,
                        CallOrigin::Builtin {
                            name: b.name(),
                            at: span,
                        },
                    ) {
                        Ok(v) => v,
                        Err(e) => {
                            self.release_all(out);
                            return Err(e);
                        }
                    };
                    if let Err(e) = self.expect_list("flat_map", mapped, span) {
                        self.pool.release_val(mapped);
                        self.release_all(out);
                        return Err(e);
                    }
                    for sub in self.list_to_vec(mapped) {
                        self.pool.retain_val(sub);
                        out.push(sub);
                    }
                    self.pool.release_val(mapped);
                }
                self.build_list_from_owned(out, span)
            }
            BuiltinId::Fold => {
                let [f, init, list_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'fold' takes 3 arguments (fn, init, list)",
                    ));
                };
                self.expect_list("fold", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                let mut acc = *init;
                self.pool.retain_val(acc);
                for x in items {
                    match self.apply(
                        *f,
                        &[acc, x],
                        span,
                        CallOrigin::Builtin {
                            name: b.name(),
                            at: span,
                        },
                    ) {
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
            BuiltinId::Zip => {
                let [a_v, b_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'zip' takes 2 arguments (list, list)",
                    ));
                };
                self.expect_list("zip", *a_v, span)?;
                self.expect_list("zip", *b_v, span)?;
                let a = self.list_to_vec(*a_v);
                let b = self.list_to_vec(*b_v);
                let mut out = Vec::with_capacity(a.len().min(b.len()));
                for (x, y) in a.into_iter().zip(b) {
                    self.pool.retain_val(x);
                    self.pool.retain_val(y);
                    match self.pool.alloc_obj(Obj::Tuple(vec![x, y]), span) {
                        Ok(id) => out.push(RVal::Obj(id)),
                        Err(e) => {
                            self.release_all(out);
                            return Err(e);
                        }
                    }
                }
                self.build_list_from_owned(out, span)
            }
            BuiltinId::Enumerate => {
                let xs = self.expect_one_list("enumerate", args, span)?;
                let items = self.list_to_vec(xs);
                let mut out = Vec::with_capacity(items.len());
                for (i, x) in items.into_iter().enumerate() {
                    self.pool.retain_val(x);
                    match self
                        .pool
                        .alloc_obj(Obj::Tuple(vec![RVal::Int(i as i64), x]), span)
                    {
                        Ok(id) => out.push(RVal::Obj(id)),
                        Err(e) => {
                            self.release_all(out);
                            return Err(e);
                        }
                    }
                }
                self.build_list_from_owned(out, span)
            }
            BuiltinId::Sort => {
                let xs = self.expect_one_list("sort", args, span)?;
                let mut items = self.list_to_vec(xs);
                let mut sort_err = None;
                items.sort_by(|&a, &b| match self.compare_vals(a, b, span) {
                    Ok(ord) => ord,
                    Err(e) => {
                        if sort_err.is_none() {
                            sort_err = Some(e);
                        }
                        std::cmp::Ordering::Equal
                    }
                });
                if let Some(e) = sort_err {
                    return Err(e);
                }
                self.build_list_from_borrowed(&items, span)
            }
            BuiltinId::SortBy => {
                let [key_fn, list_v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'sort_by' takes 2 arguments (key_fn, list)",
                    ));
                };
                self.expect_list("sort_by", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                let mut keyed = Vec::with_capacity(items.len());
                for x in items {
                    match self.apply(
                        *key_fn,
                        &[x],
                        span,
                        CallOrigin::Builtin {
                            name: b.name(),
                            at: span,
                        },
                    ) {
                        Ok(k) => keyed.push((k, x)),
                        Err(e) => {
                            for (k, _) in keyed {
                                self.pool.release_val(k);
                            }
                            return Err(e);
                        }
                    }
                }
                let mut sort_err = None;
                keyed.sort_by(|(ka, _), (kb, _)| match self.compare_vals(*ka, *kb, span) {
                    Ok(ord) => ord,
                    Err(e) => {
                        if sort_err.is_none() {
                            sort_err = Some(e);
                        }
                        std::cmp::Ordering::Equal
                    }
                });
                let sorted: Vec<RVal> = keyed.iter().map(|(_, v)| *v).collect();
                for (k, _) in keyed {
                    self.pool.release_val(k);
                }
                if let Some(e) = sort_err {
                    return Err(e);
                }
                self.build_list_from_borrowed(&sorted, span)
            }
            BuiltinId::Unique => {
                let xs = self.expect_one_list("unique", args, span)?;
                let items = self.list_to_vec(xs);
                let mut out: Vec<RVal> = Vec::new();
                for x in items {
                    if !out.iter().any(|&seen| self.values_equal(seen, x)) {
                        out.push(x);
                    }
                }
                self.build_list_from_borrowed(&out, span)
            }
            BuiltinId::ListToArray => {
                let xs = self.expect_one_list("list->array", args, span)?;
                let items = self.list_to_vec(xs);
                for &item in &items {
                    self.pool.retain_val(item);
                }
                Ok(RVal::Obj(self.pool.alloc_obj(Obj::Array(items), span)?))
            }
            BuiltinId::ArrayToList => {
                let [v] = args else {
                    return Err(Error::runtime(
                        "fn.list",
                        span,
                        "'array->list' takes 1 argument",
                    ));
                };
                let items = match v {
                    RVal::Obj(id) => match self.pool.get(*id) {
                        Obj::Array(arr) => arr.clone(),
                        _ => {
                            return Err(Error::runtime(
                                "fn.list",
                                span,
                                format!(
                                    "'array->list' expects array, got {}",
                                    v.type_name(self.pool)
                                ),
                            ));
                        }
                    },
                    other => {
                        return Err(Error::runtime(
                            "fn.list",
                            span,
                            format!(
                                "'array->list' expects array, got {}",
                                other.type_name(self.pool)
                            ),
                        ));
                    }
                };
                self.build_list_from_borrowed(&items, span)
            }
        }
    }
}
