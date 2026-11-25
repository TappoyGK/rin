//! `Builder`・`Machine` の生成・凍結・評価・ホスト呼び出しと後始末。
use super::*;

impl<'a> Builder<'a> {
    /// 読み込み段階の新しい環境を生成する。
    pub fn new(limits: Limits, alloc: &'a mut dyn Allocator) -> Self {
        Self {
            inner: EnvState::new(limits, alloc),
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

    /// ホストの値を大域変数として束縛する（同名があれば上書きし、旧値を解放する）。
    pub fn bind(&mut self, name: &str, value: &Value) -> Result<(), Error> {
        self.inner.bind(name, value)
    }

    /// ホスト提供のソースから名前付きモジュールを読み込んで評価する。
    pub fn load_module(&mut self, name: &str, source: &str) -> Result<(), Error> {
        self.inner.load_module(name, source)
    }

    /// 読み込み段階の環境でソースを評価する（`def`・`defn`・`import` 等を許可する）。
    pub fn eval(&mut self, source: &str) -> Result<Value, Error> {
        self.inner.eval_building(source)
    }

    /// 現在内部プール上で生存しているオブジェクトの概算バイト数を返す。
    pub fn live_bytes(&self) -> usize {
        self.inner.pool.live_bytes()
    }

    /// 大域環境と読み込み済みモジュールを凍結し、実行段階の [`Machine`] へ移行する。
    pub fn freeze(mut self) -> Result<Machine<'a>, Error> {
        self.check_freeze()?;
        // 凍結後のモジュール名参照を O(1) のスロット参照で行えるよう索引を作る。
        // 値は `modules` が所有するため、ここでは参照カウントを増やさない。
        for (name, &mod_val) in &self.inner.modules {
            let id = self.inner.syms.intern(name);
            set_slot(&mut self.inner.frozen_modules, id, mod_val);
        }
        Ok(Machine { inner: self.inner })
    }

    pub(crate) fn check_freeze(&self) -> Result<(), Error> {
        Ok(())
    }

    pub(crate) fn run_test(
        &mut self,
        setup: &[crate::ast::Expr],
        body: &[crate::ast::Expr],
    ) -> (Vec<crate::test::CheckFailure>, Result<(), Error>) {
        self.inner.run_test(setup, body)
    }

    pub(crate) fn call(&mut self, fn_name: &str, args: &[Value]) -> Result<Value, Error> {
        self.inner.call(fn_name, args, false)
    }

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
    pub fn eval(&mut self, source: &str) -> Result<Value, Error> {
        self.eval_with(source, &[])
    }

    /// 凍結した環境で、この 1 回の評価にだけ有効な初期束縛 `bindings` を与えてソースを評価する。
    ///
    /// 評価の成否にかかわらず、大域環境・読み込み済みモジュールは変更せず、一時束縛は終了時に解放する。
    pub fn eval_with(&mut self, source: &str, bindings: &[(&str, Value)]) -> Result<Value, Error> {
        self.inner.eval_frozen(source, bindings)
    }

    /// 定義済みの大域関数をソース再解析なしで直接呼び出す。
    pub fn call(&mut self, fn_name: &str, args: &[Value]) -> Result<Value, Error> {
        self.inner.call(fn_name, args, true)
    }

    /// 読み込み済みモジュールの公開関数をソース再解析なしで直接呼び出す。
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
    pub(super) fn new(limits: Limits, alloc: &'a mut dyn Allocator) -> Self {
        let mut syms = SymTable::default();
        let mut globals = Vec::new();
        for &b in BuiltinId::ALL {
            let id = syms.intern(b.name());
            set_slot(&mut globals, id, RVal::Builtin(b));
        }
        let pi_id = syms.intern("pi");
        set_slot(&mut globals, pi_id, RVal::Float(std::f64::consts::PI));
        Self {
            limits,
            math_fma: MathFma::default(),
            pool: Pool::new(alloc, limits.memory_bytes),
            syms,
            globals,
            modules: BTreeMap::new(),
            frozen_modules: Vec::new(),
            module_ids: BTreeMap::new(),
            module_names: Vec::new(),
            module_envs: Vec::new(),
            parse_scratch: ParseScratch::new(),
        }
    }

    pub(super) fn bind(&mut self, name: &str, value: &Value) -> Result<(), Error> {
        let rval = self.pool.import(value, Span::default())?;
        let id = self.syms.intern(name);
        if let Some(prev) = set_slot(&mut self.globals, id, rval) {
            self.pool.release_val(prev);
        }
        Ok(())
    }

    pub(super) fn eval_building(&mut self, source: &str) -> Result<Value, Error> {
        let chunk = {
            let mut scratch = ParseScratchGuard::new(&mut self.parse_scratch);
            let exprs = scratch.parse(source)?;
            compile_program(exprs, None, &mut self.syms, &self.globals, false)?
        };
        let mut runner = self.runner(None, false);
        let last = runner.run_top_chunk(&chunk)?;
        let exported = runner.pool.export(last);
        runner.pool.release_val(last);
        Ok(exported)
    }

    pub(super) fn eval_frozen(
        &mut self,
        source: &str,
        bindings: &[(&str, Value)],
    ) -> Result<Value, Error> {
        let chunk = {
            let mut scratch = ParseScratchGuard::new(&mut self.parse_scratch);
            let exprs = scratch.parse(source)?;
            compile_program(exprs, None, &mut self.syms, &self.globals, true)?
        };

        if bindings.is_empty() {
            let mut runner = self.runner(None, true);
            let last = runner.run_top_chunk(&chunk)?;
            let exported = runner.pool.export(last);
            runner.pool.release_val(last);
            return Ok(exported);
        }

        // 一時束縛を取り込み、失敗時は取り込み済みをすべて解放する。
        // 少数の束縛はスタック配列に格納し、ヒープ確保を避ける。
        let mut inline_entries = [(0u32, RVal::Void); INLINE_EVAL_BINDINGS];
        let mut inline_len = 0usize;
        let mut heap_entries = if bindings.len() > INLINE_EVAL_BINDINGS {
            Vec::with_capacity(bindings.len())
        } else {
            Vec::new()
        };
        let use_inline = bindings.len() <= INLINE_EVAL_BINDINGS;

        for (name, value) in bindings {
            if let Some(id) = self.syms.get(name) {
                match self.pool.import(value, Span::default()) {
                    Ok(rval) => {
                        let entries: &mut [(u32, RVal)] = if use_inline {
                            &mut inline_entries[..inline_len]
                        } else {
                            &mut heap_entries
                        };
                        if let Some(slot) = entries.iter_mut().find(|(k, _)| *k == id) {
                            let prev = std::mem::replace(&mut slot.1, rval);
                            self.pool.release_val(prev);
                        } else if use_inline {
                            inline_entries[inline_len] = (id, rval);
                            inline_len += 1;
                        } else {
                            heap_entries.push((id, rval));
                        }
                    }
                    Err(e) => {
                        let entries: &[(u32, RVal)] = if use_inline {
                            &inline_entries[..inline_len]
                        } else {
                            &heap_entries
                        };
                        for &(_, v) in entries {
                            self.pool.release_val(v);
                        }
                        return Err(e);
                    }
                }
            } else {
                // ソース中で参照されない束縛もメモリ上限検査だけは行うため、取り込んで即解放する。
                match self.pool.import(value, Span::default()) {
                    Ok(rval) => self.pool.release_val(rval),
                    Err(e) => {
                        let entries: &[(u32, RVal)] = if use_inline {
                            &inline_entries[..inline_len]
                        } else {
                            &heap_entries
                        };
                        for &(_, v) in entries {
                            self.pool.release_val(v);
                        }
                        return Err(e);
                    }
                }
            }
        }

        let entries: &[(u32, RVal)] = if use_inline {
            &inline_entries[..inline_len]
        } else {
            &heap_entries
        };
        let eval_bindings = EvalBindings {
            entries,
            raw: bindings,
        };
        let res = {
            let mut runner = self.runner(None, true);
            runner.eval_bindings = Some(&eval_bindings);
            runner.run_top_chunk(&chunk)
        };
        for &(_, v) in entries {
            self.pool.release_val(v);
        }
        let last = res?;
        let exported = self.pool.export(last);
        self.pool.release_val(last);
        Ok(exported)
    }

    pub(super) fn run_test(
        &mut self,
        setup: &[crate::ast::Expr],
        body: &[crate::ast::Expr],
    ) -> (Vec<crate::test::CheckFailure>, Result<(), Error>) {
        let chunk =
            match compile_program_parts(setup, body, None, &mut self.syms, &self.globals, false) {
                Ok(chunk) => chunk,
                Err(error) => return (Vec::new(), Err(error)),
            };
        let mut runner = self.runner(None, false);
        let result = runner
            .run_top_chunk(&chunk)
            .map(|value| runner.pool.release_val(value));
        (std::mem::take(&mut runner.check_failures), result)
    }

    pub(super) fn call(
        &mut self,
        fn_name: &str,
        args: &[Value],
        frozen: bool,
    ) -> Result<Value, Error> {
        let Some(callee) = self
            .syms
            .get(fn_name)
            .and_then(|id| self.globals.get(id as usize).copied().flatten())
        else {
            return Err(Error::runtime(
                "expr.name",
                Span::default(),
                suggest(
                    format!("unknown function '{fn_name}'"),
                    fn_name,
                    bound_names(&self.syms, &self.globals),
                ),
            ));
        };
        self.call_rval(callee, args, frozen)
    }

    pub(super) fn call_rval(
        &mut self,
        callee: RVal,
        args: &[Value],
        frozen: bool,
    ) -> Result<Value, Error> {
        let mut r_args = Vec::with_capacity(args.len());
        for arg in args {
            match self.pool.import(arg, Span::default()) {
                Ok(rv) => r_args.push(rv),
                Err(e) => {
                    for v in r_args {
                        self.pool.release_val(v);
                    }
                    return Err(e);
                }
            }
        }
        self.pool.retain_val(callee);
        let mut runner = self.runner(None, frozen);
        let res = runner.apply(callee, &r_args, Span::default(), CallOrigin::Host);
        runner.pool.release_val(callee);
        runner.release_all(r_args);
        let val = res?;
        let exported = runner.pool.export(val);
        runner.pool.release_val(val);
        Ok(exported)
    }

    pub(super) fn runner(&mut self, active_module: Option<u32>, frozen: bool) -> Runner<'_, 'a> {
        Runner {
            limits: self.limits,
            math_fma: self.math_fma,
            steps_left: self.limits.steps,
            depth: 0,
            frozen,
            eval_bindings: None,
            pool: &mut self.pool,
            syms: &self.syms,
            globals: &mut self.globals,
            modules: &self.modules,
            frozen_modules: if frozen { &self.frozen_modules } else { &[] },
            module_names: &self.module_names,
            module_envs: &mut self.module_envs,
            active_module,
            export_list: None,
            check_failures: Vec::new(),
            stack: Vec::with_capacity(64),
        }
    }
}

impl Drop for EnvState<'_> {
    fn drop(&mut self) {
        let old_mods = std::mem::take(&mut self.modules);
        for v in old_mods.into_values() {
            self.pool.release_val(v);
        }
        let old_mod_envs = std::mem::take(&mut self.module_envs);
        for env in old_mod_envs {
            for v in env.into_iter().flatten() {
                self.pool.release_val(v);
            }
        }
        let old = std::mem::take(&mut self.globals);
        for v in old.into_iter().flatten() {
            self.pool.release_val(v);
        }
    }
}
