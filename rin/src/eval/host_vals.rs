//! システムヒープを使わないホスト境界 API（[`Val`] 構築・[`ValueRef`] / [`ErrorRef`] 返却）。
use std::fmt::Write;

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

    pub(super) fn val_list(&mut self, items: &[Val]) -> Result<Val, ErrorRef<'_>> {
        let mut tail = RVal::EmptyList;
        for &item in items.iter().rev() {
            self.pool.retain_val(item.0);
            match self.pool.alloc_cons(item.0, tail, Span::default()) {
                Ok(rv) => tail = rv,
                Err(_) => {
                    self.pool.release_val(item.0);
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
        // 挿入ソート（システムヒープ不使用）
        let ps = pairs.as_mut_slice();
        for i in 1..ps.len() {
            let mut j = i;
            while j > 0 && ps[j - 1].0 > ps[j].0 {
                ps.swap(j - 1, j);
                j -= 1;
            }
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

    pub(super) fn value_ref(&self, v: Val) -> ValueRef<'_> {
        ValueRef::new(v.0, &self.pool)
    }

    pub(super) fn bind_val(&mut self, name: &str, val: Val) -> Result<(), ErrorRef<'_>> {
        self.pool.retain_val(val.0);
        let Some(id) = self.syms.intern(&mut self.pool, name) else {
            self.pool.release_val(val.0);
            self.pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return Err(self.pool.core.error.as_ref());
        };
        match self.globals.set(&mut self.pool, id, val.0, Span::default()) {
            Ok(Some(prev)) => {
                self.pool.release_val(prev);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(_) => {
                self.pool.release_val(val.0);
                Err(self.pool.core.error.as_ref())
            }
        }
    }

    pub(super) fn eval_building_ref(&mut self, source: &str) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.clear_last_result();
        let mark = self.pool.work.mark(&self.pool.core);
        let res = (|| {
            let exprs =
                parse_program_work(source, &mut self.pool.work, &mut self.pool.core, false)?;
            let chunk = compile_program(
                exprs.as_slice(),
                None,
                &mut self.syms,
                &mut self.pool,
                &self.globals,
                false,
            )?;
            let mut runner = self.runner(None, false);
            runner.run_top_chunk(&chunk)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        match res {
            Ok(v) => {
                self.set_last_result(v);
                Ok(ValueRef::new(self.last_result, &self.pool))
            }
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }

    pub(super) fn eval_frozen_ref(
        &mut self,
        source: &str,
        bindings: &[(&str, Val)],
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.clear_last_result();
        let outer_mark = self.pool.work.mark(&self.pool.core);
        let res = self.eval_frozen_vals_inner(source, bindings);
        self.syms.clear_temp();
        self.pool.work.rewind(&mut self.pool.core, outer_mark);
        match res {
            Ok(v) => {
                self.set_last_result(v);
                Ok(ValueRef::new(self.last_result, &self.pool))
            }
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }

    fn eval_frozen_vals_inner(
        &mut self,
        source: &str,
        bindings: &[(&str, Val)],
    ) -> Result<RVal, ErrRecorded> {
        let chunk = {
            let exprs =
                parse_program_work(source, &mut self.pool.work, &mut self.pool.core, false)?;
            compile_program(
                exprs.as_slice(),
                None,
                &mut self.syms,
                &mut self.pool,
                &self.globals,
                true,
            )?
        };
        if bindings.is_empty() {
            let mut runner = self.runner(None, true);
            return runner.run_top_chunk(&chunk);
        }
        let mut values = EvalBindingValues::new(bindings.len());
        for &(name, val) in bindings {
            if let Some(id) = self.syms.get(name) {
                self.pool.retain_val(val.0);
                match values.insert(&mut self.pool, id, val.0) {
                    Ok(Some(prev)) => self.pool.release_val(prev),
                    Ok(None) => {}
                    Err(e) => {
                        self.pool.release_val(val.0);
                        values.release_all(&mut self.pool);
                        return Err(e);
                    }
                }
            }
        }
        let eval_bindings = EvalBindings {
            values,
            names: EvalBindingNames::Vals(bindings),
        };
        let res = {
            let mut runner = self.runner(None, true);
            runner.eval_bindings = Some(&eval_bindings);
            runner.run_top_chunk(&chunk)
        };
        eval_bindings.values.release_all(&mut self.pool);
        res
    }

    pub(super) fn call_vals_ref(
        &mut self,
        fn_name: &str,
        args: &[Val],
        frozen: bool,
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.clear_last_result();
        let Some(callee) = self
            .syms
            .get(fn_name)
            .and_then(|id| global_slot(&self.globals, id))
        else {
            let out =
                self.pool
                    .core
                    .error
                    .begin_msg("expr.name", ErrorKind::Runtime, Span::default());
            let _ = write!(out, "unknown function '{fn_name}'");
            write_suggest(out, fn_name, global_bound_names(&self.syms, &self.globals));
            return Err(self.pool.core.error.as_ref());
        };
        self.call_rval_vals_ref(callee, args, frozen)
    }

    pub(super) fn call_in_vals_ref(
        &mut self,
        mod_name: &str,
        fn_name: &str,
        args: &[Val],
        frozen: bool,
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.clear_last_result();
        let Some(RVal::Obj(mod_id)) = self.modules.get_val(mod_name) else {
            let out =
                self.pool
                    .core
                    .error
                    .begin_msg("expr.module", ErrorKind::Runtime, Span::default());
            let _ = write!(out, "unknown module '{mod_name}'");
            write_suggest(out, mod_name, self.modules.loaded_names());
            return Err(self.pool.core.error.as_ref());
        };
        let Obj::Module { exports, .. } = *self.pool.get(mod_id) else {
            unreachable!();
        };
        let Some(callee) = Pool::find_module_export(exports, fn_name) else {
            let out =
                self.pool
                    .core
                    .error
                    .begin_msg("expr.module", ErrorKind::Runtime, Span::default());
            let _ = write!(out, "module '{mod_name}' has no export '{fn_name}'");
            write_available(
                out,
                "exports",
                exports.as_slice().iter().map(|e| e.name.as_str()),
            );
            return Err(self.pool.core.error.as_ref());
        };
        self.call_rval_vals_ref(callee, args, frozen)
    }

    fn call_rval_vals_ref(
        &mut self,
        callee: RVal,
        args: &[Val],
        frozen: bool,
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        let mark = self.pool.work.mark(&self.pool.core);
        let mut r_args = RawVec::new();
        for &arg in args {
            if r_args
                .push_work(&mut self.pool.work, &mut self.pool.core, arg.0)
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
        for &v in r_args.as_slice() {
            self.pool.retain_val(v);
        }
        self.pool.retain_val(callee);
        let res = {
            let mut runner = self.runner(None, frozen);
            let res = runner.apply(callee, r_args.as_slice(), Span::default(), CallOrigin::Host);
            runner.pool.release_val(callee);
            for &v in r_args.as_slice() {
                runner.pool.release_val(v);
            }
            res
        };
        self.pool.work.rewind(&mut self.pool.core, mark);
        match res {
            Ok(v) => {
                self.set_last_result(v);
                Ok(ValueRef::new(self.last_result, &self.pool))
            }
            Err(_) => Err(self.pool.core.error.as_ref()),
        }
    }
}

macro_rules! impl_val_constructors {
    ($ty:ident) => {
        impl $ty<'_> {
            pub fn val_void(&self) -> Val {
                Val(RVal::Void)
            }
            pub fn val_int(&self, n: i64) -> Val {
                Val(RVal::Int(n))
            }
            pub fn val_float(&self, x: f64) -> Val {
                Val(RVal::Float(x))
            }
            pub fn val_bool(&self, b: bool) -> Val {
                Val(RVal::Bool(b))
            }
            pub fn val_char(&self, c: char) -> Val {
                Val(RVal::Char(c))
            }
            pub fn val_none(&self) -> Val {
                Val(RVal::None)
            }
            pub fn val_str(&mut self, s: &str) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_str(s)
            }
            pub fn val_list(&mut self, items: &[Val]) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_list(items)
            }
            pub fn val_array(&mut self, items: &[Val]) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_array(items)
            }
            pub fn val_tuple(&mut self, items: &[Val]) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_tuple(items)
            }
            pub fn val_struct(
                &mut self,
                name: &str,
                fields: &[(&str, Val)],
            ) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_struct(name, fields)
            }
            pub fn val_variant(
                &mut self,
                type_name: &str,
                variant: &str,
                data: Option<Val>,
            ) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_variant(type_name, variant, data)
            }
            pub fn val_some(&mut self, v: Val) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_some(v)
            }
            pub fn val_ok(&mut self, v: Val) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_ok(v)
            }
            pub fn val_err(&mut self, v: Val) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_err(v)
            }
            pub fn release_val(&mut self, v: Val) {
                self.inner.release_val(v);
            }
            pub(crate) fn retain_val(&mut self, v: Val) {
                self.inner.retain_val(v);
            }
            pub(crate) fn last_result_val(&self) -> Val {
                self.inner.last_result_val()
            }
            pub(crate) fn val_cons(&mut self, head: Val, tail: Val) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_cons(head, tail)
            }
            pub(crate) fn val_i64_array(&mut self, items: &[i64]) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_i64_array(items)
            }
            pub(crate) fn val_f64_array(&mut self, items: &[f64]) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_f64_array(items)
            }
            pub(crate) fn val_struct_set(
                &mut self,
                s: Val,
                field: &str,
                val: Val,
            ) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_struct_set(s, field, val)
            }
            pub fn value_ref(&self, v: Val) -> ValueRef<'_> {
                self.inner.value_ref(v)
            }
        }
    };
}

impl_val_constructors!(Builder);
impl_val_constructors!(Machine);

impl Builder<'_> {
    /// マシン上で構築した [`Val`] を大域変数として束縛する（システムヒープ不使用）。
    pub fn bind_val(&mut self, name: &str, val: Val) -> Result<(), ErrorRef<'_>> {
        self.inner.bind_val(name, val)
    }

    /// モジュールを読み込み、失敗時は借用エラー [`ErrorRef`] を返す。
    pub fn load_module_ref(&mut self, name: &str, source: &str) -> Result<(), ErrorRef<'_>> {
        match self.inner.load_module_inner(name, source) {
            Ok(()) => Ok(()),
            Err(_) => Err(self.inner.pool.core.error.as_ref()),
        }
    }

    /// 読み込み段階で評価し、結果の借用ビュー [`ValueRef`] を返す（システムヒープ不使用）。
    pub fn eval_ref(&mut self, source: &str) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.inner.eval_building_ref(source)
    }

    pub(crate) fn call_ref(
        &mut self,
        fn_name: &str,
        args: &[Val],
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.inner.call_vals_ref(fn_name, args, false)
    }

    pub(crate) fn call_in_ref(
        &mut self,
        mod_name: &str,
        fn_name: &str,
        args: &[Val],
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.inner.call_in_vals_ref(mod_name, fn_name, args, false)
    }
}

impl Machine<'_> {
    /// 凍結した環境で評価し、結果の借用ビュー [`ValueRef`] を返す（システムヒープ不使用）。
    pub fn eval_ref(&mut self, source: &str) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.inner.eval_frozen_ref(source, &[])
    }

    /// 凍結した環境で一時束縛 [`Val`] を与えて評価し、借用ビュー [`ValueRef`] を返す（システムヒープ不使用）。
    pub fn eval_with_vals(
        &mut self,
        source: &str,
        bindings: &[(&str, Val)],
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.inner.eval_frozen_ref(source, bindings)
    }

    /// 定義済みの大域関数を [`Val`] 引数で呼び出し、借用ビュー [`ValueRef`] を返す（システムヒープ不使用）。
    pub fn call_ref(&mut self, fn_name: &str, args: &[Val]) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.inner.call_vals_ref(fn_name, args, true)
    }

    /// 読み込み済みモジュールの公開関数を [`Val`] 引数で呼び出し、借用ビュー [`ValueRef`] を返す（システムヒープ不使用）。
    pub fn call_in_ref(
        &mut self,
        mod_name: &str,
        fn_name: &str,
        args: &[Val],
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.inner.call_in_vals_ref(mod_name, fn_name, args, true)
    }
}
