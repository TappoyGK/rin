//! システムヒープを使わないホスト境界の評価・呼び出し・`Builder` / `Machine` 公開 API。
use std::fmt::Write;

use super::*;

impl EnvState<'_> {
    pub(super) fn value_ref(&self, v: Val) -> ValueRef<'_> {
        ValueRef::new(v.0, &self.pool)
    }

    pub(super) fn fn_sig(&self, fn_name: &str) -> Option<crate::FnSigRef<'_>> {
        let vb = self.type_env.lookup_root_val(fn_name)?;
        let sig = vb.fn_sig?;
        Some(crate::FnSigRef::new(&self.type_env, sig))
    }

    pub(super) fn fn_sig_in(&self, mod_name: &str, fn_name: &str) -> Option<crate::FnSigRef<'_>> {
        let (_, menv) = self.type_env.lookup_module(mod_name)?;
        let h = crate::mem::fnv1a_bytes(fn_name.as_bytes());
        let vb = menv.exports_vals.get_by(h, |k| k.as_str() == fn_name)?;
        let sig = vb.fn_sig?;
        Some(crate::FnSigRef::new(&self.type_env, sig))
    }

    pub(super) fn type_of(&self, name: &str) -> Option<crate::TypeRef<'_>> {
        let vb = self.type_env.lookup_root_val(name)?;
        Some(crate::TypeRef::new(&self.type_env, vb.ty))
    }

    pub(super) fn type_of_in(&self, mod_name: &str, name: &str) -> Option<crate::TypeRef<'_>> {
        let (_, menv) = self.type_env.lookup_module(mod_name)?;
        let h = crate::mem::fnv1a_bytes(name.as_bytes());
        let vb = menv.exports_vals.get_by(h, |k| k.as_str() == name)?;
        Some(crate::TypeRef::new(&self.type_env, vb.ty))
    }

    pub(super) fn resolve_type(
        &mut self,
        type_expr: &str,
    ) -> Result<crate::TypeHandle, ErrorRef<'_>> {
        let mark = self.pool.work.mark(&self.pool.core);
        let type_snap = self.type_env.snapshot();
        let res = (|| {
            let te = parse_type_expr_work(type_expr, &mut self.pool.work, &mut self.pool.core)?;
            let tid = resolve_type_expr_perm(
                &te,
                &mut self.type_env,
                &mut self.pool.perm,
                &mut self.pool.work,
                &mut self.pool.core,
            )?;
            reject_fn_in_perm_type(
                tid,
                &self.type_env,
                &mut self.pool.work,
                &mut self.pool.core,
                Span::default(),
            )?;
            self.type_env.mark_boundary_ok(
                &mut self.pool.perm,
                &mut self.pool.core,
                tid,
                Span::default(),
            )?;
            Ok::<_, ErrRecorded>(crate::TypeHandle(tid))
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        match res {
            Ok(h) => Ok(h),
            Err(_) => {
                self.type_env.rollback(type_snap);
                Err(self.pool.core.error.as_ref())
            }
        }
    }

    pub(super) fn bind_typed_val(
        &mut self,
        name: &str,
        ty: crate::TypeHandle,
        val: Val,
    ) -> Result<(), ErrorRef<'_>> {
        if !self.type_env.is_boundary_type_id(ty.0) {
            self.pool.core.err_str(
                "type.boundary",
                ErrorKind::Runtime,
                Span::default(),
                "invalid type handle",
            );
            return Err(self.pool.core.error.as_ref());
        }
        if self.modules.get_val(name).is_some() {
            self.pool.core.err_fmt(
                "expr.def",
                ErrorKind::Runtime,
                Span::default(),
                format_args!("cannot bind '{name}': conflicts with loaded module '{name}'"),
            );
            return Err(self.pool.core.error.as_ref());
        }
        let Some(id) = self.syms.intern(&mut self.pool, name) else {
            self.pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return Err(self.pool.core.error.as_ref());
        };
        if id < STATIC_SYM_COUNT {
            self.pool.core.err_fmt(
                "expr.def",
                ErrorKind::Runtime,
                Span::default(),
                format_args!("cannot redefine builtin '{name}'"),
            );
            return Err(self.pool.core.error.as_ref());
        }
        if self.globals.get(id).is_some() {
            self.pool.core.err_fmt(
                "expr.def",
                ErrorKind::Runtime,
                Span::default(),
                format_args!("'{name}' is already defined"),
            );
            return Err(self.pool.core.error.as_ref());
        }
        let mark = self.pool.work.mark(&self.pool.core);
        let check_res =
            check_rval_against_type(val.0, ty.0, &self.type_env, &mut self.pool, Span::default());
        self.pool.work.rewind(&mut self.pool.core, mark);
        if check_res.is_err() {
            return Err(self.pool.core.error.as_ref());
        }
        let type_snap = self.type_env.snapshot();
        if self
            .type_env
            .define_root_val(
                &mut self.pool.perm,
                &mut self.pool.core,
                name,
                ValBinding {
                    ty: ty.0,
                    fn_sig: None,
                },
                Span::default(),
            )
            .is_err()
        {
            self.type_env.rollback(type_snap);
            return Err(self.pool.core.error.as_ref());
        }
        self.pool.retain_val(val.0);
        match self.globals.set(&mut self.pool, id, val.0, Span::default()) {
            Ok(_) => Ok(()),
            Err(_) => {
                self.type_env.rollback(type_snap);
                self.pool.release_val(val.0);
                Err(self.pool.core.error.as_ref())
            }
        }
    }

    pub(super) fn bind_val(
        &mut self,
        name: &str,
        type_expr: &str,
        val: Val,
    ) -> Result<(), ErrorRef<'_>> {
        if self.modules.get_val(name).is_some() {
            self.pool.core.err_fmt(
                "expr.def",
                ErrorKind::Runtime,
                Span::default(),
                format_args!("cannot bind '{name}': conflicts with loaded module '{name}'"),
            );
            return Err(self.pool.core.error.as_ref());
        }
        let Some(id) = self.syms.intern(&mut self.pool, name) else {
            self.pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return Err(self.pool.core.error.as_ref());
        };
        if id < STATIC_SYM_COUNT {
            self.pool.core.err_fmt(
                "expr.def",
                ErrorKind::Runtime,
                Span::default(),
                format_args!("cannot redefine builtin '{name}'"),
            );
            return Err(self.pool.core.error.as_ref());
        }
        if self.globals.get(id).is_some() {
            self.pool.core.err_fmt(
                "expr.def",
                ErrorKind::Runtime,
                Span::default(),
                format_args!("'{name}' is already defined"),
            );
            return Err(self.pool.core.error.as_ref());
        }
        let mark = self.pool.work.mark(&self.pool.core);
        let type_snap = self.type_env.snapshot();
        let tid = match (|| {
            let te = parse_type_expr_work(type_expr, &mut self.pool.work, &mut self.pool.core)?;
            let tid = resolve_type_expr_perm(
                &te,
                &mut self.type_env,
                &mut self.pool.perm,
                &mut self.pool.work,
                &mut self.pool.core,
            )?;
            reject_fn_in_perm_type(
                tid,
                &self.type_env,
                &mut self.pool.work,
                &mut self.pool.core,
                Span::default(),
            )?;
            check_rval_against_type(val.0, tid, &self.type_env, &mut self.pool, Span::default())?;
            Ok::<_, ErrRecorded>(tid)
        })() {
            Ok::<_, ErrRecorded>(tid) => tid,
            Err(_) => {
                self.type_env.rollback(type_snap);
                self.pool.work.rewind(&mut self.pool.core, mark);
                return Err(self.pool.core.error.as_ref());
            }
        };
        self.pool.work.rewind(&mut self.pool.core, mark);
        if self
            .type_env
            .define_root_val(
                &mut self.pool.perm,
                &mut self.pool.core,
                name,
                ValBinding {
                    ty: tid,
                    fn_sig: None,
                },
                Span::default(),
            )
            .is_err()
        {
            self.type_env.rollback(type_snap);
            return Err(self.pool.core.error.as_ref());
        }
        self.pool.retain_val(val.0);
        match self.globals.set(&mut self.pool, id, val.0, Span::default()) {
            Ok(_) => Ok(()),
            Err(_) => {
                self.type_env.rollback(type_snap);
                self.pool.release_val(val.0);
                Err(self.pool.core.error.as_ref())
            }
        }
    }

    pub(super) fn eval_building_ref(&mut self, source: &str) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.clear_last_result();
        let mark = self.pool.work.mark(&self.pool.core);
        let type_snap = self.type_env.snapshot();
        let res = (|| {
            let exprs =
                parse_program_work(source, &mut self.pool.work, &mut self.pool.core, false)?;
            check_program_work(
                exprs.as_slice(),
                &mut self.type_env,
                &mut self.pool.perm,
                &mut self.pool.work,
                &mut self.pool.core,
                CheckOptions {
                    frozen: false,
                    strict_defn_annotations: true,
                    module_name: None,
                    test_mode: false,
                    commit: true,
                    extra_bindings: &[],
                    extra_binding_exprs: &[],
                },
            )?;
            let chunk = compile_program(
                exprs.as_slice(),
                None,
                &mut self.syms,
                &mut self.pool,
                &self.globals,
                false,
            )?;
            self.reserve_global_sigs_for_exprs(exprs.as_slice())?;
            let val = {
                let mut runner = self.runner(None, false);
                runner.run_top_chunk(&chunk)?
            };
            self.populate_global_sigs_for_exprs(exprs.as_slice());
            Ok::<_, ErrRecorded>(val)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        match res {
            Ok(v) => {
                self.set_last_result(v);
                Ok(ValueRef::new(self.last_result, &self.pool))
            }
            Err(_) => {
                self.type_env.rollback(type_snap);
                Err(self.pool.core.error.as_ref())
            }
        }
    }

    pub(super) fn eval_frozen_ref(
        &mut self,
        source: &str,
        bindings: &[(&str, &str, Val)],
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
        bindings: &[(&str, &str, Val)],
    ) -> Result<RVal, ErrRecorded> {
        if bindings.is_empty() {
            let chunk = {
                let exprs =
                    parse_program_work(source, &mut self.pool.work, &mut self.pool.core, false)?;
                check_program_work(
                    exprs.as_slice(),
                    &mut self.type_env,
                    &mut self.pool.perm,
                    &mut self.pool.work,
                    &mut self.pool.core,
                    CheckOptions {
                        frozen: true,
                        strict_defn_annotations: true,
                        module_name: None,
                        test_mode: false,
                        commit: false,
                        extra_bindings: &[],
                        extra_binding_exprs: &[],
                    },
                )?;
                compile_program(
                    exprs.as_slice(),
                    None,
                    &mut self.syms,
                    &mut self.pool,
                    &self.globals,
                    true,
                )?
            };
            let mut runner = self.runner(None, true);
            return runner.run_top_chunk(&chunk);
        }
        let mut extra_exprs = RawVec::<(&str, crate::ast::TypeExpr)>::new();
        for &(bname, bty, bval) in bindings {
            let te = parse_type_expr_work(bty, &mut self.pool.work, &mut self.pool.core)?;
            validate_boundary_type_expr(
                &te,
                &self.type_env,
                &mut self.pool.work,
                &mut self.pool.core,
            )?;
            check_rval_against_type_expr(
                bval.0,
                &te,
                &self.type_env,
                &mut self.pool,
                Span::default(),
            )?;
            if extra_exprs
                .push_work(&mut self.pool.work, &mut self.pool.core, (bname, te))
                .is_none()
            {
                return Err(self.pool.core.err_oom(Span::default()));
            }
        }
        let (chunk, used) = {
            let exprs =
                parse_program_work(source, &mut self.pool.work, &mut self.pool.core, false)?;
            check_program_work(
                exprs.as_slice(),
                &mut self.type_env,
                &mut self.pool.perm,
                &mut self.pool.work,
                &mut self.pool.core,
                CheckOptions {
                    frozen: true,
                    strict_defn_annotations: true,
                    module_name: None,
                    test_mode: false,
                    commit: false,
                    extra_bindings: &[],
                    extra_binding_exprs: extra_exprs.as_slice(),
                },
            )?;
            compile_program_with_bindings(
                exprs.as_slice(),
                None,
                &mut self.syms,
                &mut self.pool,
                &self.globals,
                true,
                InitialBindingNames::Vals(bindings),
            )?
        };
        match used {
            UsedBindings::Small { indices, len } => {
                let mut vals = [RVal::Void; 8];
                let count = len as usize;
                for i in 0..count {
                    vals[i] = bindings[indices[i] as usize].2.0;
                }
                let mut runner = self.runner(None, true);
                runner.run_top_chunk_with_locals(&chunk, &vals[..count])
            }
            UsedBindings::Many(used_indices) => {
                let mut vals = RawVec::<RVal>::new();
                if !used_indices.is_empty() {
                    if vals
                        .reserve_work(&mut self.pool.work, &mut self.pool.core, used_indices.len())
                        .is_none()
                    {
                        return Err(self.pool.core.err_oom(Span::default()));
                    }
                    for &idx in used_indices.as_slice() {
                        let _ = vals.push_work(
                            &mut self.pool.work,
                            &mut self.pool.core,
                            bindings[idx as usize].2.0,
                        );
                    }
                }
                let mut runner = self.runner(None, true);
                runner.run_top_chunk_with_locals(&chunk, vals.as_slice())
            }
        }
    }

    pub(super) fn eval_frozen_typed_ref(
        &mut self,
        source: &str,
        bindings: &[(&str, crate::TypeHandle, Val)],
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.clear_last_result();
        let outer_mark = self.pool.work.mark(&self.pool.core);
        let res = self.eval_frozen_typed_vals_inner(source, bindings);
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

    fn eval_frozen_typed_vals_inner(
        &mut self,
        source: &str,
        bindings: &[(&str, crate::TypeHandle, Val)],
    ) -> Result<RVal, ErrRecorded> {
        if bindings.is_empty() {
            return self.eval_frozen_vals_inner(source, &[]);
        }
        let mut extra_types = RawVec::<(&str, crate::typecheck::TypeId)>::new();
        for &(bname, bty, bval) in bindings {
            if !self.type_env.is_boundary_type_id(bty.0) {
                return Err(self.pool.core.err_str(
                    "type.boundary",
                    ErrorKind::Runtime,
                    Span::default(),
                    "invalid type handle",
                ));
            }
            check_rval_against_type(
                bval.0,
                bty.0,
                &self.type_env,
                &mut self.pool,
                Span::default(),
            )?;
            if extra_types
                .push_work(&mut self.pool.work, &mut self.pool.core, (bname, bty.0))
                .is_none()
            {
                return Err(self.pool.core.err_oom(Span::default()));
            }
        }
        let (chunk, used) = {
            let exprs =
                parse_program_work(source, &mut self.pool.work, &mut self.pool.core, false)?;
            check_program_work(
                exprs.as_slice(),
                &mut self.type_env,
                &mut self.pool.perm,
                &mut self.pool.work,
                &mut self.pool.core,
                CheckOptions {
                    frozen: true,
                    strict_defn_annotations: true,
                    module_name: None,
                    test_mode: false,
                    commit: false,
                    extra_bindings: extra_types.as_slice(),
                    extra_binding_exprs: &[],
                },
            )?;
            compile_program_with_bindings(
                exprs.as_slice(),
                None,
                &mut self.syms,
                &mut self.pool,
                &self.globals,
                true,
                InitialBindingNames::TypedVals(bindings),
            )?
        };
        match used {
            UsedBindings::Small { indices, len } => {
                let mut vals = [RVal::Void; 8];
                let count = len as usize;
                for i in 0..count {
                    vals[i] = bindings[indices[i] as usize].2.0;
                }
                let mut runner = self.runner(None, true);
                runner.run_top_chunk_with_locals(&chunk, &vals[..count])
            }
            UsedBindings::Many(used_indices) => {
                let mut vals = RawVec::<RVal>::new();
                if !used_indices.is_empty() {
                    if vals
                        .reserve_work(&mut self.pool.work, &mut self.pool.core, used_indices.len())
                        .is_none()
                    {
                        return Err(self.pool.core.err_oom(Span::default()));
                    }
                    for &idx in used_indices.as_slice() {
                        let _ = vals.push_work(
                            &mut self.pool.work,
                            &mut self.pool.core,
                            bindings[idx as usize].2.0,
                        );
                    }
                }
                let mut runner = self.runner(None, true);
                runner.run_top_chunk_with_locals(&chunk, vals.as_slice())
            }
        }
    }

    pub(super) fn call_vals_ref(
        &mut self,
        fn_name: &str,
        args: &[Val],
        frozen: bool,
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.clear_last_result();
        let Some((sym_id, callee)) = self
            .syms
            .get(fn_name)
            .and_then(|id| global_slot(&self.globals, id).map(|v| (id, v)))
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
        let fn_sig = self.global_fn_sig(sym_id);
        self.call_rval_vals_ref(callee, fn_sig, args, frozen)
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
        let fn_sig = self.type_env.lookup_module(mod_name).and_then(|(_, menv)| {
            menv.exports_vals
                .get_by(fnv1a_bytes(fn_name.as_bytes()), |k| k.as_str() == fn_name)
                .and_then(|vb| vb.fn_sig)
        });
        self.call_rval_vals_ref(callee, fn_sig, args, frozen)
    }

    fn call_rval_vals_ref(
        &mut self,
        callee: RVal,
        fn_sig: Option<FnSig>,
        args: &[Val],
        frozen: bool,
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        if let Some(sig) = fn_sig {
            let params = sig.params.as_slice();
            if params.len() != args.len() {
                self.pool.core.err_str(
                    "type.boundary",
                    ErrorKind::Runtime,
                    Span::default(),
                    "wrong number of arguments",
                );
                return Err(self.pool.core.error.as_ref());
            }
            for (arg, &(_, pty)) in args.iter().zip(params) {
                if check_rval_against_type(
                    arg.0,
                    pty,
                    &self.type_env,
                    &mut self.pool,
                    Span::default(),
                )
                .is_err()
                {
                    return Err(self.pool.core.error.as_ref());
                }
            }
        }
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
            pub fn val_text(&mut self, s: &str) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_text(s)
            }
            pub fn val_ident(&mut self, s: &str) -> Result<Val, ErrorRef<'_>> {
                self.inner.val_ident(s)
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
    /// 型式文字列を事前解決し、反復評価・束縛に再利用できる [`crate::TypeHandle`] を返す（システムヒープ不使用）。
    pub fn resolve_type(&mut self, type_expr: &str) -> Result<crate::TypeHandle, ErrorRef<'_>> {
        self.inner.resolve_type(type_expr)
    }

    /// マシン上で構築した [`Val`] を型式 `type_expr` 付きで大域変数として束縛する（システムヒープ不使用）。
    pub fn bind_val(&mut self, name: &str, type_expr: &str, val: Val) -> Result<(), ErrorRef<'_>> {
        self.inner.bind_val(name, type_expr, val)
    }

    /// 事前解決済みの [`crate::TypeHandle`] を使って [`Val`] を大域変数として束縛する（システムヒープ不使用）。
    pub fn bind_typed(
        &mut self,
        name: &str,
        ty: crate::TypeHandle,
        val: Val,
    ) -> Result<(), ErrorRef<'_>> {
        self.inner.bind_typed_val(name, ty, val)
    }

    /// 大域関数 `defn` のシグネチャを取得する（システムヒープ不使用）。
    pub fn fn_sig(&self, fn_name: &str) -> Option<crate::FnSigRef<'_>> {
        self.inner.fn_sig(fn_name)
    }

    /// 読み込み済みモジュールの公開関数 `defn` のシグネチャを取得する（システムヒープ不使用）。
    pub fn fn_sig_in(&self, mod_name: &str, fn_name: &str) -> Option<crate::FnSigRef<'_>> {
        self.inner.fn_sig_in(mod_name, fn_name)
    }

    /// 大域束縛の型を取得する（システムヒープ不使用）。
    pub fn type_of(&self, name: &str) -> Option<crate::TypeRef<'_>> {
        self.inner.type_of(name)
    }

    /// 読み込み済みモジュールの公開束縛の型を取得する（システムヒープ不使用）。
    pub fn type_of_in(&self, mod_name: &str, name: &str) -> Option<crate::TypeRef<'_>> {
        self.inner.type_of_in(mod_name, name)
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

    /// 凍結した環境で型式付き一時束縛 [`Val`] を与えて評価し、借用ビュー [`ValueRef`] を返す（システムヒープ不使用）。
    pub fn eval_with_vals(
        &mut self,
        source: &str,
        bindings: &[(&str, &str, Val)],
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.inner.eval_frozen_ref(source, bindings)
    }

    /// 凍結した環境で事前解決済み [`crate::TypeHandle`] 付き一時束縛 [`Val`] を与えて評価し、借用ビュー [`ValueRef`] を返す（システムヒープ不使用）。
    pub fn eval_with_typed_vals(
        &mut self,
        source: &str,
        bindings: &[(&str, crate::TypeHandle, Val)],
    ) -> Result<ValueRef<'_>, ErrorRef<'_>> {
        self.inner.eval_frozen_typed_ref(source, bindings)
    }

    /// 大域関数 `defn` のシグネチャを取得する（システムヒープ不使用）。
    pub fn fn_sig(&self, fn_name: &str) -> Option<crate::FnSigRef<'_>> {
        self.inner.fn_sig(fn_name)
    }

    /// 読み込み済みモジュールの公開関数 `defn` のシグネチャを取得する（システムヒープ不使用）。
    pub fn fn_sig_in(&self, mod_name: &str, fn_name: &str) -> Option<crate::FnSigRef<'_>> {
        self.inner.fn_sig_in(mod_name, fn_name)
    }

    /// 大域束縛の型を取得する（システムヒープ不使用）。
    pub fn type_of(&self, name: &str) -> Option<crate::TypeRef<'_>> {
        self.inner.type_of(name)
    }

    /// 読み込み済みモジュールの公開束縛の型を取得する（システムヒープ不使用）。
    pub fn type_of_in(&self, mod_name: &str, name: &str) -> Option<crate::TypeRef<'_>> {
        self.inner.type_of_in(mod_name, name)
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
