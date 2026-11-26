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
        self.inner.pool.freeze_perm();
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
        Self {
            limits,
            math_fma: MathFma::default(),
            pool: Pool::new(alloc, limits.memory_bytes),
            syms: SymTable::new(),
            globals: EnvSlots::new(),
            modules: ModuleTable::new(),
        }
    }

    pub(super) fn bind(&mut self, name: &str, value: &Value) -> Result<(), Error> {
        let rval = self.pool.import(value, Span::default())?;
        let Some(id) = self.syms.intern(&mut self.pool, name) else {
            self.pool.release_val(rval);
            return Err(Error::new(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            ));
        };
        match self
            .globals
            .set(&mut self.pool, id, rval, Span::default())
        {
            Ok(Some(prev)) => self.pool.release_val(prev),
            Ok(None) => {}
            Err(e) => {
                self.pool.release_val(rval);
                return Err(e);
            }
        }
        Ok(())
    }

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
        let last = res?;
        let exported = self.pool.export(last);
        self.pool.release_val(last);
        Ok(exported)
    }

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

    fn eval_frozen_inner(
        &mut self,
        source: &str,
        bindings: &[(&str, Value)],
    ) -> Result<Value, Error> {
        let chunk = {
            let exprs = parse_program_work(source, &mut self.pool.work, &mut self.pool.core, false)?;
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
            let res = {
                let mut runner = self.runner(None, true);
                runner.run_top_chunk(&chunk)
            };
            let last = res?;
            let exported = self.pool.export(last);
            self.pool.release_val(last);
            return Ok(exported);
        }

        // 一時束縛を取り込み、失敗時は取り込み済みをすべて解放する。
        // 少数の束縛はスタック配列に格納し、多数は WorkArena 上の RawMap を使う。
        let mut values = EvalBindingValues::new(bindings.len());
        for (name, value) in bindings {
            if let Some(id) = self.syms.get(name) {
                match self.pool.import(value, Span::default()) {
                    Ok(rval) => match values.insert(&mut self.pool, id, rval) {
                        Ok(Some(prev)) => self.pool.release_val(prev),
                        Ok(None) => {}
                        Err(e) => {
                            self.pool.release_val(rval);
                            values.release_all(&mut self.pool);
                            return Err(e);
                        }
                    },
                    Err(e) => {
                        values.release_all(&mut self.pool);
                        return Err(e);
                    }
                }
            } else {
                // ソース中で参照されない束縛もメモリ上限検査だけは行うため、取り込んで即解放する。
                match self.pool.import(value, Span::default()) {
                    Ok(rval) => self.pool.release_val(rval),
                    Err(e) => {
                        values.release_all(&mut self.pool);
                        return Err(e);
                    }
                }
            }
        }

        let eval_bindings = EvalBindings {
            values,
            raw: bindings,
        };
        let res = {
            let mut runner = self.runner(None, true);
            runner.eval_bindings = Some(&eval_bindings);
            runner.run_top_chunk(&chunk)
        };
        eval_bindings.values.release_all(&mut self.pool);
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
        let mark = self.pool.work.mark(&self.pool.core);
        let chunk = match compile_program_parts(
            setup,
            body,
            None,
            &mut self.syms,
            &mut self.pool,
            &self.globals,
            false,
        ) {
            Ok(chunk) => chunk,
            Err(error) => {
                self.pool.work.rewind(&mut self.pool.core, mark);
                return (Vec::new(), Err(error));
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
        (failures, result)
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
            .and_then(|id| global_slot(&self.globals, id))
        else {
            return Err(Error::runtime(
                "expr.name",
                Span::default(),
                suggest(
                    format!("unknown function '{fn_name}'"),
                    fn_name,
                    global_bound_names(&self.syms, &self.globals),
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
        let mark = self.pool.work.mark(&self.pool.core);
        let mut r_args = RawVec::new();
        if !args.is_empty()
            && r_args
                .reserve_work(&mut self.pool.work, &mut self.pool.core, args.len())
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
                    if r_args
                        .push_work(&mut self.pool.work, &mut self.pool.core, rv)
                        .is_none()
                    {
                        self.pool.release_val(rv);
                        for &v in r_args.as_slice() {
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
                Err(e) => {
                    for &v in r_args.as_slice() {
                        self.pool.release_val(v);
                    }
                    self.pool.work.rewind(&mut self.pool.core, mark);
                    return Err(e);
                }
            }
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
        let val = res?;
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
            eval_bindings: None,
            pool: &mut self.pool,
            syms: &self.syms,
            globals: &mut self.globals,
            modules: &mut self.modules,
            active_module,
            export_list: None,
            check_failures: Vec::new(),
            stack: ValueStack::new(),
        }
    }
}

impl Drop for EnvState<'_> {
    fn drop(&mut self) {
        self.modules.release_all(&mut self.pool);
        self.globals.clear(&mut self.pool);
    }
}
