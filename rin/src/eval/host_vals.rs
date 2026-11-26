//! システムヒープを使わないホスト境界の値構築・参照カウント管理。

use super::*;

impl EnvState<'_> {
    pub(super) fn set_last_result(&mut self, val: RVal) {
        let prev = std::mem::replace(&mut self.last_result, val);
        self.pool.release_val(prev);
    }

    pub(super) fn clear_last_result(&mut self) {
        let prev = std::mem::replace(&mut self.last_result, RVal::Void);
        self.pool.release_val(prev);
    }

    pub(super) fn val_str(&mut self, s: &str) -> Result<Val, ErrorRef<'_>> {
        match self.pool.alloc_str(s, Span::default()) {
            Ok(id) => Ok(Val(RVal::Obj(id))),
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }

    pub(super) fn val_ident(&mut self, s: &str) -> Result<Val, ErrorRef<'_>> {
        if !crate::lexer::is_valid_ident(s) {
            self.pool.core.err_str(
                "host.value",
                ErrorKind::Runtime,
                Span::default(),
                "invalid identifier",
            );
            return Err(self.pool.core.error.as_ref());
        }
        match self.pool.alloc_ident(s, Span::default()) {
            Ok(id) => Ok(Val(RVal::Obj(id))),
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }

    pub(super) fn val_list(&mut self, items: &[Val]) -> Result<Val, ErrorRef<'_>> {
        let mut tail = RVal::EmptyList;
        for &item in items.iter().rev() {
            match self.pool.alloc_cons(item.0, tail, Span::default()) {
                Ok(rv) => {
                    self.pool.release_val(tail);
                    tail = rv;
                }
                Err(_) => {
                    self.pool.release_val(tail);
                    return Err(self.pool.core.error.as_ref());
                }
            }
        }
        Ok(Val(tail))
    }

    pub(super) fn val_array(&mut self, items: &[Val]) -> Result<Val, ErrorRef<'_>> {
        let mark = self.pool.work.mark(&self.pool.core);
        let mut rvals = RawVec::new();
        for &item in items {
            if rvals
                .push_work(&mut self.pool.work, &mut self.pool.core, item.0)
                .is_none()
            {
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    Span::default(),
                    "memory limit exceeded",
                );
                return Err(self.pool.core.error.as_ref());
            }
        }
        for &v in rvals.as_slice() {
            self.pool.retain_val(v);
        }
        let res = self
            .pool
            .alloc_array_owned(rvals.as_slice(), Span::default());
        self.pool.work.rewind(&mut self.pool.core, mark);
        match res {
            Ok(id) => Ok(Val(RVal::Obj(id))),
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }

    pub(super) fn val_tuple(&mut self, items: &[Val]) -> Result<Val, ErrorRef<'_>> {
        let mark = self.pool.work.mark(&self.pool.core);
        let mut rvals = RawVec::new();
        for &item in items {
            if rvals
                .push_work(&mut self.pool.work, &mut self.pool.core, item.0)
                .is_none()
            {
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    Span::default(),
                    "memory limit exceeded",
                );
                return Err(self.pool.core.error.as_ref());
            }
        }
        for &v in rvals.as_slice() {
            self.pool.retain_val(v);
        }
        let res = self
            .pool
            .alloc_tuple_owned(rvals.as_slice(), Span::default());
        self.pool.work.rewind(&mut self.pool.core, mark);
        match res {
            Ok(id) => Ok(Val(RVal::Obj(id))),
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }

    pub(super) fn val_struct(
        &mut self,
        name: &str,
        fields: &[(&str, Val)],
    ) -> Result<Val, ErrorRef<'_>> {
        let mark = self.pool.work.mark(&self.pool.core);
        let mut pairs: RawVec<(&str, RVal)> = RawVec::new();
        for &(k, v) in fields {
            if pairs
                .push_work(&mut self.pool.work, &mut self.pool.core, (k, v.0))
                .is_none()
            {
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    Span::default(),
                    "memory limit exceeded",
                );
                return Err(self.pool.core.error.as_ref());
            }
        }
        // 安定マージソート（O(F log F)、システムヒープ不使用）
        if sort_field_pairs_work(
            pairs.as_mut_slice(),
            &mut self.pool.work,
            &mut self.pool.core,
        )
        .is_none()
        {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return Err(self.pool.core.error.as_ref());
        }
        // 重複キーは後勝ちで上書きする。
        let mut unique_keys: RawVec<&str> = RawVec::new();
        let mut unique_vals: RawVec<RVal> = RawVec::new();
        for &(k, v) in pairs.as_slice() {
            if unique_keys.last().copied() == Some(k) {
                *unique_vals.last_mut().unwrap() = v;
            } else if unique_keys
                .push_work(&mut self.pool.work, &mut self.pool.core, k)
                .is_none()
                || unique_vals
                    .push_work(&mut self.pool.work, &mut self.pool.core, v)
                    .is_none()
            {
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    Span::default(),
                    "memory limit exceeded",
                );
                return Err(self.pool.core.error.as_ref());
            }
        }
        let desc = match self
            .pool
            .intern_struct_desc(name, unique_keys.as_slice(), Span::default())
        {
            Ok(d) => d,
            Err(_) => {
                self.pool.work.rewind(&mut self.pool.core, mark);
                return Err(self.pool.core.error.as_ref());
            }
        };
        for &v in unique_vals.as_slice() {
            self.pool.retain_val(v);
        }
        let res = self
            .pool
            .alloc_struct_owned(desc, unique_vals.as_slice(), Span::default());
        self.pool.work.rewind(&mut self.pool.core, mark);
        match res {
            Ok(id) => Ok(Val(RVal::Obj(id))),
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }

    pub(super) fn val_variant(
        &mut self,
        type_name: &str,
        variant: &str,
        data: Option<Val>,
    ) -> Result<Val, ErrorRef<'_>> {
        let data_rval = data.map(|v| {
            self.pool.retain_val(v.0);
            v.0
        });
        match self
            .pool
            .alloc_variant_owned(type_name, variant, data_rval, Span::default())
        {
            Ok(id) => Ok(Val(RVal::Obj(id))),
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }

    pub(super) fn val_some(&mut self, v: Val) -> Result<Val, ErrorRef<'_>> {
        self.pool.retain_val(v.0);
        match self.pool.alloc_obj(Obj::Some(v.0), Span::default()) {
            Ok(id) => Ok(Val(RVal::Obj(id))),
            Err(_) => {
                self.pool.release_val(v.0);
                Err(self.pool.core.error.as_ref())
            }
        }
    }

    pub(super) fn val_ok(&mut self, v: Val) -> Result<Val, ErrorRef<'_>> {
        self.pool.retain_val(v.0);
        match self.pool.alloc_obj(Obj::Ok(v.0), Span::default()) {
            Ok(id) => Ok(Val(RVal::Obj(id))),
            Err(_) => {
                self.pool.release_val(v.0);
                Err(self.pool.core.error.as_ref())
            }
        }
    }

    pub(super) fn val_err(&mut self, v: Val) -> Result<Val, ErrorRef<'_>> {
        self.pool.retain_val(v.0);
        match self.pool.alloc_obj(Obj::Err(v.0), Span::default()) {
            Ok(id) => Ok(Val(RVal::Obj(id))),
            Err(_) => {
                self.pool.release_val(v.0);
                Err(self.pool.core.error.as_ref())
            }
        }
    }

    pub(super) fn release_val(&mut self, v: Val) {
        self.pool.release_val(v.0);
    }

    pub(super) fn retain_val(&mut self, v: Val) {
        self.pool.retain_val(v.0);
    }

    pub(super) fn last_result_val(&self) -> Val {
        Val(self.last_result)
    }

    pub(super) fn val_cons(&mut self, head: Val, tail: Val) -> Result<Val, ErrorRef<'_>> {
        let is_list = matches!(tail.0, RVal::EmptyList)
            || matches!(tail.0, RVal::Obj(id) if matches!(self.pool.get(id), Obj::Cons { .. }));
        if !is_list {
            self.pool.core.err_str(
                "fn.list",
                ErrorKind::Runtime,
                Span::default(),
                "'cons' expects list tail",
            );
            return Err(self.pool.core.error.as_ref());
        }
        match self.pool.alloc_cons(head.0, tail.0, Span::default()) {
            Ok(rv) => Ok(Val(rv)),
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }

    pub(super) fn val_i64_array(&mut self, items: &[i64]) -> Result<Val, ErrorRef<'_>> {
        let mark = self.pool.work.mark(&self.pool.core);
        let mut rvals = RawVec::new();
        for &n in items {
            if rvals
                .push_work(&mut self.pool.work, &mut self.pool.core, RVal::Int(n))
                .is_none()
            {
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    Span::default(),
                    "memory limit exceeded",
                );
                return Err(self.pool.core.error.as_ref());
            }
        }
        let res = self
            .pool
            .alloc_array_owned(rvals.as_slice(), Span::default());
        self.pool.work.rewind(&mut self.pool.core, mark);
        match res {
            Ok(id) => Ok(Val(RVal::Obj(id))),
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }

    pub(super) fn val_f64_array(&mut self, items: &[f64]) -> Result<Val, ErrorRef<'_>> {
        let mark = self.pool.work.mark(&self.pool.core);
        let mut rvals = RawVec::new();
        for &x in items {
            if rvals
                .push_work(&mut self.pool.work, &mut self.pool.core, RVal::Float(x))
                .is_none()
            {
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    Span::default(),
                    "memory limit exceeded",
                );
                return Err(self.pool.core.error.as_ref());
            }
        }
        let res = self
            .pool
            .alloc_array_owned(rvals.as_slice(), Span::default());
        self.pool.work.rewind(&mut self.pool.core, mark);
        match res {
            Ok(id) => Ok(Val(RVal::Obj(id))),
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }

    pub(super) fn val_struct_set(
        &mut self,
        s: Val,
        field: &str,
        val: Val,
    ) -> Result<Val, ErrorRef<'_>> {
        let RVal::Obj(id) = s.0 else {
            self.pool.core.err_str(
                "expr.with",
                ErrorKind::Runtime,
                Span::default(),
                "expected struct value",
            );
            return Err(self.pool.core.error.as_ref());
        };
        let Obj::Struct { desc, values } = *self.pool.get(id) else {
            self.pool.core.err_str(
                "expr.with",
                ErrorKind::Runtime,
                Span::default(),
                "expected struct value",
            );
            return Err(self.pool.core.error.as_ref());
        };
        let mark = self.pool.work.mark(&self.pool.core);
        let Some(name_raw) = self.pool.copy_struct_desc_name_to_work(desc) else {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return Err(self.pool.core.error.as_ref());
        };
        let count = values.len();
        let mut pairs: RawVec<(RawStr, RVal)> = RawVec::new();
        for i in 0..count {
            let Some(fname_raw) = self.pool.copy_struct_desc_field_to_work(desc, i) else {
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    Span::default(),
                    "memory limit exceeded",
                );
                return Err(self.pool.core.error.as_ref());
            };
            if pairs
                .push_work(
                    &mut self.pool.work,
                    &mut self.pool.core,
                    (fname_raw, values[i]),
                )
                .is_none()
            {
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    Span::default(),
                    "memory limit exceeded",
                );
                return Err(self.pool.core.error.as_ref());
            }
        }
        let field_raw = match RawStr::from_work(&mut self.pool.work, &mut self.pool.core, field) {
            Some(r) => r,
            None => {
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    Span::default(),
                    "memory limit exceeded",
                );
                return Err(self.pool.core.error.as_ref());
            }
        };
        if let Some(pos) = pairs
            .as_slice()
            .iter()
            .position(|(k, _)| k.as_str() == field)
        {
            pairs.as_mut_slice()[pos].1 = val.0;
        } else if pairs
            .push_work(&mut self.pool.work, &mut self.pool.core, (field_raw, val.0))
            .is_none()
        {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return Err(self.pool.core.error.as_ref());
        }
        let ps = pairs.as_mut_slice();
        for i in 1..ps.len() {
            let mut j = i;
            while j > 0 && ps[j - 1].0.as_str() > ps[j].0.as_str() {
                ps.swap(j - 1, j);
                j -= 1;
            }
        }
        let mut key_strs: RawVec<&str> = RawVec::new();
        let mut val_rvals: RawVec<RVal> = RawVec::new();
        for (k, v) in pairs.as_slice() {
            if key_strs
                .push_work(&mut self.pool.work, &mut self.pool.core, k.as_str())
                .is_none()
                || val_rvals
                    .push_work(&mut self.pool.work, &mut self.pool.core, *v)
                    .is_none()
            {
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    Span::default(),
                    "memory limit exceeded",
                );
                return Err(self.pool.core.error.as_ref());
            }
        }
        let new_desc = match self.pool.intern_struct_desc(
            name_raw.as_str(),
            key_strs.as_slice(),
            Span::default(),
        ) {
            Ok(d) => d,
            Err(_) => {
                self.pool.work.rewind(&mut self.pool.core, mark);
                return Err(self.pool.core.error.as_ref());
            }
        };
        for &v in val_rvals.as_slice() {
            self.pool.retain_val(v);
        }
        let res = self
            .pool
            .alloc_struct_owned(new_desc, val_rvals.as_slice(), Span::default());
        self.pool.work.rewind(&mut self.pool.core, mark);
        match res {
            Ok(new_id) => Ok(Val(RVal::Obj(new_id))),
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }
}
