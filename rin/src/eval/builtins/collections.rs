//! コレクションの参照・構築・変換（一時領域はすべて作業領域 `WorkArena` を使用）。
use super::*;
use crate::mem::RawVec;

impl Runner<'_, '_> {
    pub(super) fn builtin_len(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let [v] = args else {
            return Err(self.err_runtime("fn.list", span, "'len' takes 1 argument"));
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
                    return Ok(RVal::Int(s.as_str().chars().count() as i64));
                }
                _ => {}
            }
        }
        let tname = v.type_name(self.pool);
        Err(self.err_runtime_fmt(
            "fn.list",
            span,
            format_args!("'len' expects list, array, or string, got {tname}"),
        ))
    }

    pub(super) fn builtin_is_empty(
        &mut self,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let xs = self.expect_one_list("is_empty", args, span)?;
        Ok(RVal::Bool(self.pool.list_len(xs) == 0))
    }

    pub(super) fn builtin_get(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let [list_v, idx_v] = args else {
            return Err(self.err_runtime(
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

    pub(super) fn builtin_car(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let xs = self.expect_one_list("car", args, span)?;
        match self.list_head(xs) {
            Some(head) => {
                self.pool.retain_val(head);
                Ok(RVal::Obj(self.pool.alloc_obj(Obj::Some(head), span)?))
            }
            None => Ok(RVal::None),
        }
    }

    pub(super) fn builtin_cdr(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let xs = self.expect_one_list("cdr", args, span)?;
        let tail = self.list_tail(xs);
        self.pool.retain_val(tail);
        Ok(tail)
    }

    pub(super) fn builtin_last(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let xs = self.expect_one_list("last", args, span)?;
        let len = self.pool.list_len(xs);
        if !self.try_charge(len) {
            return self.step_limit_result(span);
        }
        let mut last_val = None;
        let mut cur = xs;
        while let RVal::Obj(id) = cur {
            let Obj::Cons { head, tail, .. } = *self.pool.get(id) else {
                break;
            };
            last_val = Some(head);
            cur = tail;
        }
        match last_val {
            Some(v) => {
                self.pool.retain_val(v);
                Ok(RVal::Obj(self.pool.alloc_obj(Obj::Some(v), span)?))
            }
            None => Ok(RVal::None),
        }
    }

    pub(super) fn builtin_cons(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let [item, list_v] = args else {
            return Err(self.err_runtime(
                "fn.list",
                span,
                "'cons' takes 2 arguments (item, list)",
            ));
        };
        self.expect_list("cons", *list_v, span)?;
        self.pool.alloc_cons(*item, *list_v, span)
    }

    pub(super) fn builtin_push(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let [list_v, item] = args else {
            return Err(self.err_runtime(
                "fn.list",
                span,
                "'push' takes 2 arguments (list, item)",
            ));
        };
        self.expect_list("push", *list_v, span)?;
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let mut items = self.list_to_work_vec(*list_v, span)?;
            if items
                .push_work(&mut self.pool.work, &mut self.pool.core, *item)
                .is_none()
            {
                return Err(self.work_oom(span));
            }
            self.build_list_from_borrowed(items.as_slice(), span)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }

    pub(super) fn builtin_append(
        &mut self,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        if !self.try_charge(args.len()) {
            return self.step_limit_result(span);
        }
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let mut items = RawVec::<RVal>::new();
            for &arg in args {
                self.expect_list("append", arg, span)?;
                let sub = self.list_to_work_vec(arg, span)?;
                if items
                    .extend_from_slice_work(&mut self.pool.work, &mut self.pool.core, sub.as_slice())
                    .is_none()
                {
                    return Err(self.work_oom(span));
                }
            }
            self.build_list_from_borrowed(items.as_slice(), span)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }

    pub(super) fn builtin_flatten(
        &mut self,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let xs = self.expect_one_list("flatten", args, span)?;
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let outer = self.list_to_work_vec(xs, span)?;
            let mut items = RawVec::<RVal>::new();
            for &sub in outer.as_slice() {
                self.expect_list("flatten", sub, span)?;
                let subitems = self.list_to_work_vec(sub, span)?;
                if items
                    .extend_from_slice_work(
                        &mut self.pool.work,
                        &mut self.pool.core,
                        subitems.as_slice(),
                    )
                    .is_none()
                {
                    return Err(self.work_oom(span));
                }
            }
            self.build_list_from_borrowed(items.as_slice(), span)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }

    pub(super) fn builtin_slice(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let [list_v, start_v, end_v] = args else {
            return Err(self.err_runtime(
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
            return Err(self.err_runtime_fmt(
                "fn.list",
                span,
                format_args!("slice range {start}..{end} out of range (length {len})"),
            ));
        }
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let items = self.list_to_work_vec(*list_v, span)?;
            self.build_list_from_borrowed(&items.as_slice()[start as usize..end as usize], span)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }

    pub(super) fn builtin_take(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let [list_v, n_v] = args else {
            return Err(self.err_runtime(
                "fn.list",
                span,
                "'take' takes 2 arguments (list, n)",
            ));
        };
        self.expect_list("take", *list_v, span)?;
        let n = self.expect_int("take", *n_v, span)?;
        if n < 0 {
            return Err(self.err_runtime(
                "fn.list",
                span,
                "'take' count cannot be negative",
            ));
        }
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let items = self.list_to_work_vec(*list_v, span)?;
            let end = (n as usize).min(items.len());
            self.build_list_from_borrowed(&items.as_slice()[..end], span)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }

    pub(super) fn builtin_drop(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let [list_v, n_v] = args else {
            return Err(self.err_runtime(
                "fn.list",
                span,
                "'drop' takes 2 arguments (list, n)",
            ));
        };
        self.expect_list("drop", *list_v, span)?;
        let mut n = self.expect_int("drop", *n_v, span)?;
        if n < 0 {
            return Err(self.err_runtime(
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

    pub(super) fn builtin_reverse(
        &mut self,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
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

    pub(super) fn builtin_range(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let [start_v, end_v] = args else {
            return Err(self.err_runtime(
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

    pub(super) fn builtin_contains(
        &mut self,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let [list_v, target] = args else {
            return Err(self.err_runtime(
                "fn.list",
                span,
                "'contains' takes 2 arguments (list, value)",
            ));
        };
        self.expect_list("contains", *list_v, span)?;
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let items = self.list_to_work_vec(*list_v, span)?;
            for &x in items.as_slice() {
                let tx = x.type_name(self.pool);
                let tt = target.type_name(self.pool);
                if tx != tt {
                    return Err(self.err_runtime_fmt(
                        "fn.list",
                        span,
                        format_args!("cannot compare {tx} and {tt}"),
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
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }

    pub(super) fn builtin_zip(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let [a_v, b_v] = args else {
            return Err(self.err_runtime(
                "fn.list",
                span,
                "'zip' takes 2 arguments (list, list)",
            ));
        };
        self.expect_list("zip", *a_v, span)?;
        self.expect_list("zip", *b_v, span)?;
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let a = self.list_to_work_vec(*a_v, span)?;
            let b = self.list_to_work_vec(*b_v, span)?;
            let n = a.len().min(b.len());
            let mut out = RawVec::<RVal>::new();
            if out
                .reserve_work(&mut self.pool.work, &mut self.pool.core, n)
                .is_none()
            {
                return Err(self.work_oom(span));
            }
            for i in 0..n {
                let x = a.as_slice()[i];
                let y = b.as_slice()[i];
                self.pool.retain_val(x);
                self.pool.retain_val(y);
                match self.pool.alloc_tuple_owned(&[x, y], span) {
                    Ok(id) => {
                        let _ =
                            out.push_work(&mut self.pool.work, &mut self.pool.core, RVal::Obj(id));
                    }
                    Err(e) => {
                        self.release_slice(out.as_slice());
                        return Err(e);
                    }
                }
            }
            self.build_list_from_owned_raw(out.as_slice(), span)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }

    pub(super) fn builtin_enumerate(
        &mut self,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let xs = self.expect_one_list("enumerate", args, span)?;
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let items = self.list_to_work_vec(xs, span)?;
            let mut out = RawVec::<RVal>::new();
            if out
                .reserve_work(&mut self.pool.work, &mut self.pool.core, items.len())
                .is_none()
            {
                return Err(self.work_oom(span));
            }
            for (i, &x) in items.as_slice().iter().enumerate() {
                self.pool.retain_val(x);
                match self
                    .pool
                    .alloc_tuple_owned(&[RVal::Int(i as i64), x], span)
                {
                    Ok(id) => {
                        let _ =
                            out.push_work(&mut self.pool.work, &mut self.pool.core, RVal::Obj(id));
                    }
                    Err(e) => {
                        self.release_slice(out.as_slice());
                        return Err(e);
                    }
                }
            }
            self.build_list_from_owned_raw(out.as_slice(), span)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }

    pub(super) fn builtin_sort(&mut self, args: &[RVal], span: Span) -> Result<RVal, ErrRecorded> {
        let xs = self.expect_one_list("sort", args, span)?;
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let items = self.list_to_work_vec(xs, span)?;
            let n = items.len();
            if !self.charge_sort(items.as_slice().iter().copied(), n) {
                return self.step_limit_result(span);
            }
            let mut keyed = RawVec::<(RVal, RVal)>::new();
            if keyed
                .reserve_work(&mut self.pool.work, &mut self.pool.core, n)
                .is_none()
            {
                return Err(self.work_oom(span));
            }
            for &x in items.as_slice() {
                let _ = keyed.push_work(&mut self.pool.work, &mut self.pool.core, (x, x));
            }
            self.sort_keyed_work(keyed.as_mut_slice(), span)?;
            let mut sorted = RawVec::<RVal>::new();
            if sorted
                .reserve_work(&mut self.pool.work, &mut self.pool.core, n)
                .is_none()
            {
                return Err(self.work_oom(span));
            }
            for &(_, v) in keyed.as_slice() {
                let _ = sorted.push_work(&mut self.pool.work, &mut self.pool.core, v);
            }
            self.build_list_from_borrowed(sorted.as_slice(), span)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }

    pub(super) fn builtin_unique(
        &mut self,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let xs = self.expect_one_list("unique", args, span)?;
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let items = self.list_to_work_vec(xs, span)?;
            let mut out = RawVec::<RVal>::new();
            for &x in items.as_slice() {
                if !matches!(x, RVal::Obj(_)) {
                    let allowed = out.len().min(self.steps_left);
                    let found = out.as_slice()[..allowed]
                        .iter()
                        .position(|&v| Self::scalars_equal(v, x));
                    self.steps_left -= found.map_or(allowed, |i| i + 1);
                    if found.is_some() {
                        continue;
                    }
                    if allowed < out.len() {
                        return self.step_limit_result(span);
                    }
                    if out
                        .push_work(&mut self.pool.work, &mut self.pool.core, x)
                        .is_none()
                    {
                        return Err(self.work_oom(span));
                    }
                    continue;
                }
                let mut found = false;
                for &seen in out.as_slice() {
                    let Some(equal) = self.values_equal(seen, x) else {
                        return self.step_limit_result(span);
                    };
                    if equal {
                        found = true;
                        break;
                    }
                }
                if !found
                    && out
                        .push_work(&mut self.pool.work, &mut self.pool.core, x)
                        .is_none()
                {
                    return Err(self.work_oom(span));
                }
            }
            self.build_list_from_borrowed(out.as_slice(), span)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }

    pub(super) fn builtin_list_to_array(
        &mut self,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let xs = self.expect_one_list("list->array", args, span)?;
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let items = self.list_to_work_vec(xs, span)?;
            for &item in items.as_slice() {
                self.pool.retain_val(item);
            }
            Ok(RVal::Obj(
                self.pool.alloc_array_owned(items.as_slice(), span)?,
            ))
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        res
    }

    pub(super) fn builtin_array_to_list(
        &mut self,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
        let [v] = args else {
            return Err(self.err_runtime(
                "fn.list",
                span,
                "'array->list' takes 1 argument",
            ));
        };
        let arr_slice = match v {
            RVal::Obj(id) => match *self.pool.get(*id) {
                Obj::Array(arr) => {
                    if !super::super::runner::try_charge_steps(&mut self.steps_left, arr.len()) {
                        return self.step_limit_result(span);
                    }
                    arr
                }
                _ => {
                    let tname = v.type_name(self.pool);
                    return Err(self.err_runtime_fmt(
                        "fn.list",
                        span,
                        format_args!("'array->list' expects array, got {tname}"),
                    ));
                }
            },
            other => {
                let tname = other.type_name(self.pool);
                return Err(self.err_runtime_fmt(
                    "fn.list",
                    span,
                    format_args!("'array->list' expects array, got {tname}"),
                ));
            }
        };
        self.build_list_from_borrowed(arr_slice.as_slice(), span)
    }
}
