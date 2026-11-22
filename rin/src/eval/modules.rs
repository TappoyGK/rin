//! ホスト提供モジュールの読み込みとモジュール内の呼び出し。
use super::*;

impl Machine<'_> {
    /// ホスト提供のソースから名前付きモジュールを読み込んで評価する。
    pub fn load_module(&mut self, name: &str, source: &str) -> Result<(), Error> {
        let with_mod = |mut err: Error| {
            err.resolve_module(Some(name.to_string()));
            err
        };
        let mod_id = match self.module_ids.get(name).copied() {
            Some(id) => id,
            None => {
                let id = self.module_envs.len() as u32;
                self.module_ids.insert(name.to_string(), id);
                self.module_names.push(name.to_string());
                self.module_envs.push(Vec::new());
                id
            }
        };
        let chunk = {
            let mut scratch = ParseScratchGuard::new(&mut self.parse_scratch);
            let exprs = scratch.parse(source).map_err(with_mod)?;
            compile_program(exprs, Some(mod_id), &mut self.syms, &self.globals).map_err(with_mod)?
        };
        for v in std::mem::take(&mut self.module_envs[mod_id as usize])
            .into_iter()
            .flatten()
        {
            self.pool.release_val(v);
        }
        let mut runner = self.runner(Some(mod_id));
        let v = runner.run_top_chunk(&chunk).map_err(with_mod)?;
        runner.pool.release_val(v);
        let export_filter = runner.export_list.take();
        let mod_env = &self.module_envs[mod_id as usize];
        let mut exports = BTreeMap::new();
        match export_filter {
            Some(names) => {
                for sym in names {
                    let Some(val) = self
                        .syms
                        .get(&sym)
                        .and_then(|id| mod_env.get(id as usize).copied().flatten())
                    else {
                        for v in exports.into_values() {
                            self.pool.release_val(v);
                        }
                        return Err(Error::runtime(
                            "expr.module",
                            Span::default(),
                            format!("module '{name}' cannot export unknown symbol '{sym}'"),
                        ));
                    };
                    self.pool.retain_val(val);
                    exports.insert(Rc::from(sym.as_str()), val);
                }
            }
            None => {
                for (id, val_opt) in mod_env.iter().enumerate() {
                    let Some(val) = *val_opt else { continue };
                    let is_module = matches!(val, RVal::Obj(id) if matches!(self.pool.get(id), Obj::Module { .. }));
                    if !is_module {
                        self.pool.retain_val(val);
                        exports.insert(Rc::clone(&self.syms.names[id]), val);
                    }
                }
            }
        }
        let mod_id = self.pool.alloc_obj(
            Obj::Module {
                name: Rc::from(name),
                exports,
            },
            Span::default(),
        )?;
        if let Some(prev) = self.modules.insert(name.to_string(), RVal::Obj(mod_id)) {
            self.pool.release_val(prev);
        }
        Ok(())
    }

    /// 読み込み済みモジュールの公開関数をソース再解析なしで直接呼び出す。
    pub fn call_in(
        &mut self,
        mod_name: &str,
        fn_name: &str,
        args: &[Value],
    ) -> Result<Value, Error> {
        let Some(&RVal::Obj(mod_id)) = self.modules.get(mod_name) else {
            return Err(Error::runtime(
                "expr.module",
                Span::default(),
                suggest(
                    format!("unknown module '{mod_name}'"),
                    mod_name,
                    self.modules.keys().map(String::as_str),
                ),
            ));
        };
        let Obj::Module { exports, .. } = self.pool.get(mod_id) else {
            unreachable!();
        };
        let Some(&callee) = exports.get(fn_name) else {
            return Err(Error::runtime(
                "expr.module",
                Span::default(),
                available(
                    format!("module '{mod_name}' has no export '{fn_name}'"),
                    "exports",
                    exports.keys().map(|name| name.as_ref()),
                ),
            ));
        };
        self.call_rval(callee, args)
    }
}
