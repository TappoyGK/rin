//! Rinの関数を呼び戻す高階コレクション操作。
use super::*;

impl Runner<'_, '_> {
    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(super) fn builtin_find(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let b = BuiltinId::Find;

        let [f, list_v] = args else {
            return Self::callback_arity_error(span, "'find' takes 2 arguments (fn, list)");
        };
        self.expect_list("find", *list_v, span)?;
        let Some(items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
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

    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(super) fn builtin_any(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let b = BuiltinId::Any;

        let [f, list_v] = args else {
            return Self::callback_arity_error(span, "'any' takes 2 arguments (fn, list)");
        };
        self.expect_list("any", *list_v, span)?;
        let Some(items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
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

    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(super) fn builtin_all(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let b = BuiltinId::All;

        let [f, list_v] = args else {
            return Self::callback_arity_error(span, "'all' takes 2 arguments (fn, list)");
        };
        self.expect_list("all", *list_v, span)?;
        let Some(items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
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

    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(super) fn builtin_map(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let b = BuiltinId::Map;

        let [f, list_v] = args else {
            return Self::callback_arity_error(span, "'map' takes 2 arguments (fn, list)");
        };
        self.expect_list("map", *list_v, span)?;
        let Some(items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
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

    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(super) fn builtin_filter(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let b = BuiltinId::Filter;

        let [f, list_v] = args else {
            return Self::callback_arity_error(span, "'filter' takes 2 arguments (fn, list)");
        };
        self.expect_list("filter", *list_v, span)?;
        let Some(items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
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

    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(super) fn builtin_flat_map(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let b = BuiltinId::FlatMap;

        let [f, list_v] = args else {
            return Self::callback_arity_error(span, "'flat_map' takes 2 arguments (fn, list)");
        };
        self.expect_list("flat_map", *list_v, span)?;
        let Some(items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
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
            let subs = match self.list_to_vec(mapped) {
                Some(subs) => subs,
                None => {
                    self.pool.release_val(mapped);
                    self.release_all(out);
                    return self.step_limit_result(span);
                }
            };
            for sub in subs {
                self.pool.retain_val(sub);
                out.push(sub);
            }
            self.pool.release_val(mapped);
        }
        self.build_list_from_owned(out, span)
    }

    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(super) fn builtin_fold(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let b = BuiltinId::Fold;

        let [f, init, list_v] = args else {
            return Self::callback_arity_error(span, "'fold' takes 3 arguments (fn, init, list)");
        };
        self.expect_list("fold", *list_v, span)?;
        let Some(items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
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

    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(super) fn builtin_sort_by(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let b = BuiltinId::SortBy;

        let [key_fn, list_v] = args else {
            return Self::callback_arity_error(span, "'sort_by' takes 2 arguments (key_fn, list)");
        };
        self.expect_list("sort_by", *list_v, span)?;
        let Some(items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
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
        if !self.charge_sort(keyed.iter().map(|(k, _)| *k), keyed.len()) {
            for (k, _) in keyed {
                self.pool.release_val(k);
            }
            return self.step_limit_result(span);
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
}

impl Runner<'_, '_> {
    // 再帰中に残る高階関数のフレームから、引数エラーの整形を外す。
    #[cold]
    #[inline(never)]
    fn callback_arity_error(span: Span, message: &'static str) -> Result<RVal, Error> {
        Err(Error::runtime("fn.list", span, message))
    }
}
