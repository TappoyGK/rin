//! コレクションの参照・構築・変換。
use super::*;

impl Runner<'_, '_> {
    pub(super) fn builtin_len(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [v] = args else {
            return Err(Error::runtime("fn.list", span, "'len' takes 1 argument"));
        };
        if self.is_list(*v) {
            return Ok(RVal::Int(self.pool.list_len(*v) as i64));
        }
        if let RVal::Obj(id) = v {
            match self.pool.get(*id) {
                Obj::Array(arr) => return Ok(RVal::Int(arr.len() as i64)),
                Obj::Str(s) => {
                    if !super::super::runner::try_charge_steps(&mut self.steps_left, s.len()) {
                        return self.step_limit_result(span);
                    }
                    return Ok(RVal::Int(s.chars().count() as i64));
                }
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

    pub(super) fn builtin_is_empty(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let xs = self.expect_one_list("is_empty", args, span)?;
        Ok(RVal::Bool(self.pool.list_len(xs) == 0))
    }

    pub(super) fn builtin_get(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
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
        match self.list_get(*list_v, idx as usize, span)? {
            Some(item) => {
                self.pool.retain_val(item);
                Ok(RVal::Obj(self.pool.alloc_obj(Obj::Some(item), span)?))
            }
            None => Ok(RVal::None),
        }
    }

    pub(super) fn builtin_car(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let xs = self.expect_one_list("car", args, span)?;
        match self.list_head(xs) {
            Some(head) => {
                self.pool.retain_val(head);
                Ok(RVal::Obj(self.pool.alloc_obj(Obj::Some(head), span)?))
            }
            None => Ok(RVal::None),
        }
    }

    pub(super) fn builtin_cdr(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let xs = self.expect_one_list("cdr", args, span)?;
        let tail = self.list_tail(xs);
        self.pool.retain_val(tail);
        Ok(tail)
    }

    pub(super) fn builtin_last(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let xs = self.expect_one_list("last", args, span)?;
        let Some(items) = self.list_to_vec(xs) else {
            return self.step_limit_result(span);
        };
        match items.last().copied() {
            Some(v) => {
                self.pool.retain_val(v);
                Ok(RVal::Obj(self.pool.alloc_obj(Obj::Some(v), span)?))
            }
            None => Ok(RVal::None),
        }
    }

    pub(super) fn builtin_cons(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
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

    pub(super) fn builtin_push(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [list_v, item] = args else {
            return Err(Error::runtime(
                "fn.list",
                span,
                "'push' takes 2 arguments (list, item)",
            ));
        };
        self.expect_list("push", *list_v, span)?;
        let Some(mut items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
        items.push(*item);
        self.build_list_from_borrowed(&items, span)
    }

    pub(super) fn builtin_append(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        if !self.try_charge(args.len()) {
            return self.step_limit_result(span);
        }
        let mut items = Vec::new();
        for &arg in args {
            self.expect_list("append", arg, span)?;
            let Some(subitems) = self.list_to_vec(arg) else {
                return self.step_limit_result(span);
            };
            items.extend(subitems);
        }
        self.build_list_from_borrowed(&items, span)
    }

    pub(super) fn builtin_flatten(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let xs = self.expect_one_list("flatten", args, span)?;
        let Some(outer) = self.list_to_vec(xs) else {
            return self.step_limit_result(span);
        };
        let mut items = Vec::new();
        for sub in outer {
            self.expect_list("flatten", sub, span)?;
            let Some(subitems) = self.list_to_vec(sub) else {
                return self.step_limit_result(span);
            };
            items.extend(subitems);
        }
        self.build_list_from_borrowed(&items, span)
    }

    pub(super) fn builtin_slice(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
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
        let Some(items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
        self.build_list_from_borrowed(&items[start as usize..end as usize], span)
    }

    pub(super) fn builtin_take(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
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
        let Some(items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
        let end = (n as usize).min(items.len());
        self.build_list_from_borrowed(&items[..end], span)
    }

    pub(super) fn builtin_drop(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
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
        if !self.try_charge((n as usize).min(self.pool.list_len(*list_v))) {
            return self.step_limit_result(span);
        }
        let mut cur = *list_v;
        while n > 0 && self.pool.list_len(cur) > 0 {
            cur = self.list_tail(cur);
            n -= 1;
        }
        self.pool.retain_val(cur);
        Ok(cur)
    }

    pub(super) fn builtin_reverse(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let xs = self.expect_one_list("reverse", args, span)?;
        if !self.try_charge(self.pool.list_len(xs)) {
            return self.step_limit_result(span);
        }
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

    pub(super) fn builtin_range(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
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
        let count = usize::try_from(i128::from(end) - i128::from(start))
            .map_err(|_| self.step_limit_err(span))?;
        if !self.try_charge(count) {
            return self.step_limit_result(span);
        }
        let mut acc = RVal::EmptyList;
        for n in (start..end).rev() {
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

    pub(super) fn builtin_contains(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [list_v, target] = args else {
            return Err(Error::runtime(
                "fn.list",
                span,
                "'contains' takes 2 arguments (list, value)",
            ));
        };
        self.expect_list("contains", *list_v, span)?;
        let Some(items) = self.list_to_vec(*list_v) else {
            return self.step_limit_result(span);
        };
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
            let Some(equal) = self.values_equal(x, *target) else {
                return self.step_limit_result(span);
            };
            if equal {
                return Ok(RVal::Bool(true));
            }
        }
        Ok(RVal::Bool(false))
    }

    pub(super) fn builtin_zip(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [a_v, b_v] = args else {
            return Err(Error::runtime(
                "fn.list",
                span,
                "'zip' takes 2 arguments (list, list)",
            ));
        };
        self.expect_list("zip", *a_v, span)?;
        self.expect_list("zip", *b_v, span)?;
        let Some(a) = self.list_to_vec(*a_v) else {
            return self.step_limit_result(span);
        };
        let Some(b) = self.list_to_vec(*b_v) else {
            return self.step_limit_result(span);
        };
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

    pub(super) fn builtin_enumerate(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let xs = self.expect_one_list("enumerate", args, span)?;
        let Some(items) = self.list_to_vec(xs) else {
            return self.step_limit_result(span);
        };
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

    pub(super) fn builtin_sort(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let xs = self.expect_one_list("sort", args, span)?;
        let Some(mut items) = self.list_to_vec(xs) else {
            return self.step_limit_result(span);
        };
        if !self.charge_sort(items.iter().copied(), items.len()) {
            return self.step_limit_result(span);
        }
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

    pub(super) fn builtin_unique(&mut self, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let xs = self.expect_one_list("unique", args, span)?;
        let Some(items) = self.list_to_vec(xs) else {
            return self.step_limit_result(span);
        };
        let mut out: Vec<RVal> = Vec::new();
        for x in items {
            // スカラー比較は1組1ステップ。残量を走査の上限にして、
            // 実際に比較した数だけをまとめて計上する。
            if !matches!(x, RVal::Obj(_)) {
                let allowed = out.len().min(self.steps_left);
                let found = out[..allowed]
                    .iter()
                    .position(|&v| Self::scalars_equal(v, x));
                self.steps_left -= found.map_or(allowed, |i| i + 1);
                if found.is_some() {
                    continue;
                }
                if allowed < out.len() {
                    return self.step_limit_result(span);
                }
                out.push(x);
                continue;
            }
            let mut found = false;
            for &seen in &out {
                let Some(equal) = self.values_equal(seen, x) else {
                    return self.step_limit_result(span);
                };
                if equal {
                    found = true;
                    break;
                }
            }
            if !found {
                out.push(x);
            }
        }
        self.build_list_from_borrowed(&out, span)
    }

    pub(super) fn builtin_list_to_array(
        &mut self,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, Error> {
        let xs = self.expect_one_list("list->array", args, span)?;
        let Some(items) = self.list_to_vec(xs) else {
            return self.step_limit_result(span);
        };
        for &item in &items {
            self.pool.retain_val(item);
        }
        Ok(RVal::Obj(self.pool.alloc_obj(Obj::Array(items), span)?))
    }

    pub(super) fn builtin_array_to_list(
        &mut self,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, Error> {
        let [v] = args else {
            return Err(Error::runtime(
                "fn.list",
                span,
                "'array->list' takes 1 argument",
            ));
        };
        let items = match v {
            RVal::Obj(id) => match self.pool.get(*id) {
                Obj::Array(arr) => {
                    if !super::super::runner::try_charge_steps(&mut self.steps_left, arr.len()) {
                        return self.step_limit_result(span);
                    }
                    arr.clone()
                }
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
