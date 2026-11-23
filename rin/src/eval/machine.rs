//! Machineの生成・評価・ホスト呼び出しと後始末。
use super::*;

impl<'a> Machine<'a> {
    pub fn new(limits: Limits, alloc: &'a mut dyn Allocator) -> Self {
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

    pub fn bind(&mut self, name: &str, value: &Value) -> Result<(), Error> {
        let rval = self.pool.import(value, Span::default())?;
        let id = self.syms.intern(name);
        if let Some(prev) = set_slot(&mut self.globals, id, rval) {
            self.pool.release_val(prev);
        }
        Ok(())
    }

    pub fn live_bytes(&self) -> usize {
        self.pool.live_bytes()
    }

    pub fn eval(&mut self, source: &str) -> Result<Value, Error> {
        let chunk = {
            let mut scratch = ParseScratchGuard::new(&mut self.parse_scratch);
            let exprs = scratch.parse(source)?;
            compile_program(exprs, None, &mut self.syms, &self.globals)?
        };
        let mut runner = self.runner(None);
        let last = runner.run_top_chunk(&chunk)?;
        let exported = runner.pool.export(last);
        runner.pool.release_val(last);
        Ok(exported)
    }

    pub(crate) fn run_test(
        &mut self,
        setup: &[crate::ast::Expr],
        body: &[crate::ast::Expr],
    ) -> (Vec<crate::test::CheckFailure>, Result<(), Error>) {
        let chunk = match compile_program_parts(setup, body, None, &mut self.syms, &self.globals) {
            Ok(chunk) => chunk,
            Err(error) => return (Vec::new(), Err(error)),
        };
        let mut runner = self.runner(None);
        let result = runner
            .run_top_chunk(&chunk)
            .map(|value| runner.pool.release_val(value));
        (std::mem::take(&mut runner.check_failures), result)
    }

    /// 定義済みの大域関数をソース再解析なしで直接呼び出す。
    pub fn call(&mut self, fn_name: &str, args: &[Value]) -> Result<Value, Error> {
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
        self.call_rval(callee, args)
    }

    pub(super) fn call_rval(&mut self, callee: RVal, args: &[Value]) -> Result<Value, Error> {
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
        let mut runner = self.runner(None);
        let res = runner.apply(callee, &r_args, Span::default(), CallOrigin::Host);
        runner.pool.release_val(callee);
        runner.release_all(r_args);
        let val = res?;
        let exported = runner.pool.export(val);
        runner.pool.release_val(val);
        Ok(exported)
    }
}

impl Drop for Machine<'_> {
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

impl<'a> Machine<'a> {
    pub(super) fn runner(&mut self, active_module: Option<u32>) -> Runner<'_, 'a> {
        Runner {
            limits: self.limits,
            steps_left: self.limits.steps,
            depth: 0,
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
