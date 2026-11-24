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
    pub fn freeze(self) -> Result<Machine<'a>, Error> {
        Ok(Machine { inner: self.inner })
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
            compile_program(exprs, None, &mut self.syms, &self.globals)?
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
            validate_frozen_ast(exprs)?;
            compile_program(exprs, None, &mut self.syms, &self.globals)?
        };

        // 一時束縛を取り込み、失敗時は取り込み済みをすべて解放する。
        let mut eval_bindings = BTreeMap::new();
        let mut eval_binding_names = Vec::with_capacity(bindings.len());
        for (name, value) in bindings {
            eval_binding_names.push((*name).to_string());
            if let Some(id) = self.syms.get(name) {
                match self.pool.import(value, Span::default()) {
                    Ok(rval) => {
                        if let Some(prev) = eval_bindings.insert(id, rval) {
                            self.pool.release_val(prev);
                        }
                    }
                    Err(e) => {
                        for v in eval_bindings.into_values() {
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
                        for v in eval_bindings.into_values() {
                            self.pool.release_val(v);
                        }
                        return Err(e);
                    }
                }
            }
        }

        let mut runner = self.runner(None, true);
        runner.eval_bindings = eval_bindings;
        runner.eval_binding_names = eval_binding_names;
        let res = runner.run_top_chunk(&chunk);
        let bindings_to_release = std::mem::take(&mut runner.eval_bindings);
        for v in bindings_to_release.into_values() {
            runner.pool.release_val(v);
        }
        let last = res?;
        let exported = runner.pool.export(last);
        runner.pool.release_val(last);
        Ok(exported)
    }

    pub(super) fn run_test(
        &mut self,
        setup: &[crate::ast::Expr],
        body: &[crate::ast::Expr],
    ) -> (Vec<crate::test::CheckFailure>, Result<(), Error>) {
        let chunk = match compile_program_parts(setup, body, None, &mut self.syms, &self.globals) {
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
            eval_bindings: BTreeMap::new(),
            eval_binding_names: Vec::new(),
            pool: &mut self.pool,
            syms: &self.syms,
            globals: &mut self.globals,
            modules: &self.modules,
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
