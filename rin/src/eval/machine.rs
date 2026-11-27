//! `Builder`・`Machine` の生成・凍結・評価・ホスト呼び出しと後始末。
#[cfg(feature = "std-value")]
use std::fmt::Write;

use super::*;

impl<'a> Builder<'a> {
    /// 読み込み段階の新しい環境を生成する。
    pub fn new(limits: Limits, alloc: &'a mut dyn Allocator) -> Self {
        Self::with_memory_config(limits, MemoryConfig::default(), alloc)
    }

    /// 3 領域のブロック設定 (`MemoryConfig`) を指定して読み込み段階の環境を生成する。
    pub fn with_memory_config(
        limits: Limits,
        memory: MemoryConfig,
        alloc: &'a mut dyn Allocator,
    ) -> Self {
        Self {
            inner: EnvState::with_memory_config(limits, memory, alloc),
        }
    }

    /// 組み込み数学関数で積和融合（FMA）を使うかを設定する。既定は [`MathFma::Strict`]。
    pub fn set_math_fma(&mut self, mode: MathFma) {
        self.inner.math_fma = mode;
    }

    /// 現在の組み込み数学関数の FMA 設定を返す。
    pub fn math_fma(&self) -> MathFma {
        self.inner.math_fma
    }

    /// ホストの値を大域変数として束縛する（既存名・組み込み名・読み込み済みモジュール名との衝突はエラー）。
    #[cfg(feature = "std-value")]
    pub fn bind(&mut self, name: &str, value: &Value) -> Result<(), Error> {
        self.inner.bind(name, value)
    }

    /// ホスト提供のソースから名前付きモジュールを読み込んで評価する。
    #[cfg(feature = "std-value")]
    pub fn load_module(&mut self, name: &str, source: &str) -> Result<(), Error> {
        self.inner.load_module(name, source)
    }

    /// 読み込み段階の環境でソースを評価する（`def`・`defn`・`import` 等を許可する）。
    #[cfg(feature = "std-value")]
    pub fn eval(&mut self, source: &str) -> Result<Value, Error> {
        self.inner.eval_building(source)
    }

    /// 現在内部プール上で生存しているオブジェクトの概算バイト数を返す。
    pub fn live_bytes(&self) -> usize {
        self.inner.pool.live_bytes()
    }

    /// 大域環境と読み込み済みモジュールを凍結し、実行段階の [`Machine`] へ移行する（失敗しない・システムヒープ不使用）。
    pub fn into_machine(mut self) -> Machine<'a> {
        self.inner.pool.freeze_perm();
        Machine { inner: self.inner }
    }

    /// 大域環境と読み込み済みモジュールを凍結し、実行段階の [`Machine`] へ移行する。
    #[cfg(feature = "std-value")]
    pub fn freeze(self) -> Result<Machine<'a>, Error> {
        self.check_freeze()?;
        Ok(self.into_machine())
    }

    #[cfg(feature = "std-value")]
    pub(crate) fn check_freeze(&self) -> Result<(), Error> {
        Ok(())
    }

    #[cfg(feature = "std-value")]
    pub(crate) fn run_test(
        &mut self,
        setup: &[crate::ast::Expr],
        body: &[crate::ast::Expr],
    ) -> (Vec<crate::test::CheckFailure>, Result<(), Error>) {
        self.inner.run_test(setup, body)
    }

    #[cfg(all(test, feature = "std-value"))]
    pub(crate) fn call_in(
        &mut self,
        mod_name: &str,
        fn_name: &str,
        args: &[Value],
    ) -> Result<Value, Error> {
        self.inner.call_in(mod_name, fn_name, args, false)
    }
}

impl<'a> Machine<'a> {
    /// 組み込み数学関数で積和融合（FMA）を使うかを設定する。既定は [`MathFma::Strict`]。
    pub fn set_math_fma(&mut self, mode: MathFma) {
        self.inner.math_fma = mode;
    }

    /// 現在の組み込み数学関数の FMA 設定を返す。
    pub fn math_fma(&self) -> MathFma {
        self.inner.math_fma
    }

    /// 凍結した環境でソースを評価する。
    ///
    /// `def`・`defn`・`import`・`export` を含むソースは評価前に `[env.frozen]` の構文エラーとする。
    /// 読み込み済みのすべてのモジュールは `import` なしで `mod_name.symbol` として参照できる。
    #[cfg(feature = "std-value")]
    pub fn eval(&mut self, source: &str) -> Result<Value, Error> {
        self.eval_with(source, &[])
    }

    /// 凍結した環境で、この 1 回の評価にだけ有効な初期束縛 `bindings` を与えてソースを評価する。
    ///
    /// 評価の成否にかかわらず、大域環境・読み込み済みモジュールは変更せず、一時束縛は終了時に解放する。
    #[cfg(feature = "std-value")]
    pub fn eval_with(&mut self, source: &str, bindings: &[(&str, Value)]) -> Result<Value, Error> {
        self.inner.eval_frozen(source, bindings)
    }

    /// 定義済みの大域関数をソース再解析なしで直接呼び出す。
    #[cfg(feature = "std-value")]
    pub fn call(&mut self, fn_name: &str, args: &[Value]) -> Result<Value, Error> {
        self.inner.call(fn_name, args, true)
    }

    /// 読み込み済みモジュールの公開関数をソース再解析なしで直接呼び出す。
    #[cfg(feature = "std-value")]
    pub fn call_in(
        &mut self,
        mod_name: &str,
        fn_name: &str,
        args: &[Value],
    ) -> Result<Value, Error> {
        self.inner.call_in(mod_name, fn_name, args, true)
    }

    /// 現在内部プール上で生存しているオブジェクトの概算バイト数を返す。
    pub fn live_bytes(&self) -> usize {
        self.inner.pool.live_bytes()
    }
}

impl<'a> EnvState<'a> {
    pub(super) fn with_memory_config(
        limits: Limits,
        memory: MemoryConfig,
        alloc: &'a mut dyn Allocator,
    ) -> Self {
        Self {
            limits,
            math_fma: MathFma::default(),
            pool: Pool::with_config(alloc, limits.memory_bytes, memory),
            syms: SymTable::new(),
            globals: EnvSlots::new(),
            modules: ModuleTable::new(),
            last_result: RVal::Void,
        }
    }

    #[cfg(feature = "std-value")]
    pub(super) fn bind(&mut self, name: &str, value: &Value) -> Result<(), Error> {
        if self.modules.get_val(name).is_some() {
            return Err(Error::new(
                "expr.def",
                ErrorKind::Runtime,
                Span::default(),
                format!("cannot bind '{name}': conflicts with loaded module '{name}'"),
            ));
        }
        let Some(id) = self.syms.intern(&mut self.pool, name) else {
            return Err(Error::new(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            ));
        };
        if id < STATIC_SYM_COUNT {
            return Err(Error::new(
                "expr.def",
                ErrorKind::Runtime,
                Span::default(),
                format!("cannot redefine builtin '{name}'"),
            ));
        }
        if self.globals.get(id).is_some() {
            return Err(Error::new(
                "expr.def",
                ErrorKind::Runtime,
                Span::default(),
                format!("'{name}' is already defined"),
            ));
        }
        let rval = self
            .pool
            .import(value, Span::default())
            .map_err(|_| self.pool.core.error.to_owned_error())?;
        if self
            .globals
            .set(&mut self.pool, id, rval, Span::default())
            .is_err()
        {
            self.pool.release_val(rval);
            return Err(self.pool.core.error.to_owned_error());
        }
        Ok(())
    }

    #[cfg(feature = "std-value")]
    pub(super) fn eval_building(&mut self, source: &str) -> Result<Value, Error> {
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
        let last = match res {
            Ok(v) => v,
            Err(_) => return Err(self.pool.core.error.to_owned_error()),
        };
        let exported = self.pool.export(last);
        self.pool.release_val(last);
        Ok(exported)
    }

    #[cfg(feature = "std-value")]
    pub(super) fn eval_frozen(
        &mut self,
        source: &str,
        bindings: &[(&str, Value)],
    ) -> Result<Value, Error> {
        let outer_mark = self.pool.work.mark(&self.pool.core);
        let res = self.eval_frozen_inner(source, bindings);
        self.syms.clear_temp();
        self.pool.work.rewind(&mut self.pool.core, outer_mark);
        res
    }

    #[cfg(feature = "std-value")]
    fn eval_frozen_inner(
        &mut self,
        source: &str,
        bindings: &[(&str, Value)],
    ) -> Result<Value, Error> {
        let (chunk, used_indices) = match (|| {
            let exprs =
                parse_program_work(source, &mut self.pool.work, &mut self.pool.core, false)?;
            compile_program_with_bindings(
                exprs.as_slice(),
                None,
                &mut self.syms,
                &mut self.pool,
                &self.globals,
                true,
                InitialBindingNames::Values(bindings),
            )
        })() {
            Ok(pair) => pair,
            Err(_) => return Err(self.pool.core.error.to_owned_error()),
        };

        let mut used_vals = RawVec::<RVal>::new();
        let mut extra_objs = RawVec::<RVal>::new();
        let used_slice = used_indices.as_slice();
        if !used_slice.is_empty()
            && used_vals
                .resize_work(
                    &mut self.pool.work,
                    &mut self.pool.core,
                    used_slice.len(),
                    RVal::Void,
                )
                .is_none()
        {
            let _ = self.pool.core.err_oom(Span::default());
            return Err(self.pool.core.error.to_owned_error());
        }

        // `bindings.len() <= 8` のとき `used_slice` は `0..len`（昇順）、
        // `bindings.len() > 8` のとき `used_slice` は厳密降順。
        let ascending = used_slice.len() == bindings.len() && bindings.len() <= 8;
        let mut next_slot = if ascending { 0 } else { used_slice.len() };
        for (idx, (_, value)) in bindings.iter().enumerate() {
            match self.pool.import(value, Span::default()) {
                Ok(rval) => {
                    if ascending {
                        used_vals.as_mut_slice()[next_slot] = rval;
                        next_slot += 1;
                    } else if next_slot > 0 && used_slice[next_slot - 1] as usize == idx {
                        next_slot -= 1;
                        used_vals.as_mut_slice()[next_slot] = rval;
                    } else if matches!(rval, RVal::Obj(_))
                        && extra_objs
                            .push_work(&mut self.pool.work, &mut self.pool.core, rval)
                            .is_none()
                    {
                        self.pool.release_val(rval);
                        for &v in used_vals.as_slice() {
                            self.pool.release_val(v);
                        }
                        for &v in extra_objs.as_slice() {
                            self.pool.release_val(v);
                        }
                        let _ = self.pool.core.err_oom(Span::default());
                        return Err(self.pool.core.error.to_owned_error());
                    }
                }
                Err(_) => {
                    for &v in used_vals.as_slice() {
                        self.pool.release_val(v);
                    }
                    for &v in extra_objs.as_slice() {
                        self.pool.release_val(v);
                    }
                    return Err(self.pool.core.error.to_owned_error());
                }
            }
        }

        let res = {
            let mut runner = self.runner(None, true);
            runner.run_top_chunk_with_locals(&chunk, used_vals.as_slice())
        };
        for &v in used_vals.as_slice() {
            self.pool.release_val(v);
        }
        for &v in extra_objs.as_slice() {
            self.pool.release_val(v);
        }
        let last = match res {
            Ok(v) => v,
            Err(_) => return Err(self.pool.core.error.to_owned_error()),
        };
        let exported = self.pool.export(last);
        self.pool.release_val(last);
        Ok(exported)
    }

    #[cfg(feature = "std-value")]
    pub(super) fn run_test(
        &mut self,
        setup: &[crate::ast::Expr],
        body: &[crate::ast::Expr],
    ) -> (Vec<crate::test::CheckFailure>, Result<(), Error>) {
        let mark = self.pool.work.mark(&self.pool.core);
        let chunk = match compile_program_parts(
            setup,
            body,
            None,
            &mut self.syms,
            &mut self.pool,
            &self.globals,
            false,
            InitialBindingNames::Empty,
        ) {
            Ok((chunk, _)) => chunk,
            Err(_) => {
                self.pool.work.rewind(&mut self.pool.core, mark);
                return (Vec::new(), Err(self.pool.core.error.to_owned_error()));
            }
        };
        let (failures, result) = {
            let mut runner = self.runner(None, false);
            let result = runner
                .run_top_chunk(&chunk)
                .map(|value| runner.pool.release_val(value));
            (std::mem::take(&mut runner.check_failures), result)
        };
        self.pool.work.rewind(&mut self.pool.core, mark);
        let result = result.map_err(|_| self.pool.core.error.to_owned_error());
        (failures, result)
    }

    #[cfg(feature = "std-value")]
    pub(super) fn call(
        &mut self,
        fn_name: &str,
        args: &[Value],
        frozen: bool,
    ) -> Result<Value, Error> {
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
            return Err(self.pool.core.error.to_owned_error());
        };
        self.call_rval(callee, args, frozen)
    }

    #[cfg(feature = "std-value")]
    pub(super) fn call_rval(
        &mut self,
        callee: RVal,
        args: &[Value],
        frozen: bool,
    ) -> Result<Value, Error> {
        let mark = self.pool.work.mark(&self.pool.core);
        let mut inline_args = [RVal::Void; 4];
        let mut overflow_args = RawVec::new();
        let args_len = args.len();
        if args_len <= inline_args.len() {
            for (idx, arg) in args.iter().enumerate() {
                match self.pool.import(arg, Span::default()) {
                    Ok(rv) => inline_args[idx] = rv,
                    Err(_) => {
                        for &v in &inline_args[..idx] {
                            self.pool.release_val(v);
                        }
                        self.pool.work.rewind(&mut self.pool.core, mark);
                        return Err(self.pool.core.error.to_owned_error());
                    }
                }
            }
        } else {
            if overflow_args
                .reserve_work(&mut self.pool.work, &mut self.pool.core, args_len)
                .is_none()
            {
                self.pool.work.rewind(&mut self.pool.core, mark);
                return Err(Error::new(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    Span::default(),
                    "memory limit exceeded",
                ));
            }
            for arg in args {
                match self.pool.import(arg, Span::default()) {
                    Ok(rv) => {
                        if overflow_args
                            .push_work(&mut self.pool.work, &mut self.pool.core, rv)
                            .is_none()
                        {
                            self.pool.release_val(rv);
                            for &v in overflow_args.as_slice() {
                                self.pool.release_val(v);
                            }
                            self.pool.work.rewind(&mut self.pool.core, mark);
                            return Err(Error::new(
                                "eval.limits",
                                ErrorKind::TooMuchMemory,
                                Span::default(),
                                "memory limit exceeded",
                            ));
                        }
                    }
                    Err(_) => {
                        for &v in overflow_args.as_slice() {
                            self.pool.release_val(v);
                        }
                        self.pool.work.rewind(&mut self.pool.core, mark);
                        return Err(self.pool.core.error.to_owned_error());
                    }
                }
            }
        }
        let r_args: &[RVal] = if args_len <= inline_args.len() {
            &inline_args[..args_len]
        } else {
            overflow_args.as_slice()
        };
        self.pool.retain_val(callee);
        let res = {
            let mut runner = self.runner(None, frozen);
            let res = runner.apply(callee, r_args, Span::default(), CallOrigin::Host);
            runner.pool.release_val(callee);
            for &v in r_args {
                runner.pool.release_val(v);
            }
            res
        };
        self.pool.work.rewind(&mut self.pool.core, mark);
        let val = match res {
            Ok(v) => v,
            Err(_) => return Err(self.pool.core.error.to_owned_error()),
        };
        let exported = self.pool.export(val);
        self.pool.release_val(val);
        Ok(exported)
    }

    pub(super) fn runner(&mut self, active_module: Option<u32>, frozen: bool) -> Runner<'_, 'a> {
        Runner {
            limits: self.limits,
            math_fma: self.math_fma,
            steps_left: self.limits.steps,
            depth: 0,
            frozen,
            pool: &mut self.pool,
            syms: &self.syms,
            globals: &mut self.globals,
            modules: &mut self.modules,
            active_module,
            export_list: None,
            added_globals: RawVec::new(),
            #[cfg(feature = "std-value")]
            check_failures: Vec::new(),
            stack: ValueStack::new(),
        }
    }
}

impl Drop for EnvState<'_> {
    fn drop(&mut self) {
        let last = std::mem::replace(&mut self.last_result, RVal::Void);
        self.pool.release_val(last);
        self.modules.release_all(&mut self.pool);
        self.globals.clear(&mut self.pool);
    }
}
