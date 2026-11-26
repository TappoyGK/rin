//! ホスト提供モジュールの読み込みとモジュール内の呼び出し。
use super::*;

impl EnvState<'_> {
    /// ホスト提供のソースから名前付きモジュールを読み込んで評価する。
    pub(super) fn load_module(&mut self, name: &str, source: &str) -> Result<(), Error> {
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
        let mark = self.pool.work.mark(&self.pool.core);
        let chunk = (|| {
            let exprs = parse_program_work(source, &mut self.pool.work, &mut self.pool.core, false)
                .map_err(with_mod)?;
            compile_program(
                exprs.as_slice(),
                Some(mod_id),
                &mut self.syms,
                &self.globals,
                false,
            )
            .map_err(with_mod)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        let chunk = chunk?;
        for v in std::mem::take(&mut self.module_envs[mod_id as usize])
            .into_iter()
            .flatten()
        {
            self.pool.release_val(v);
        }
        let mod_id_u32 = mod_id;
        let mark = self.pool.work.mark(&self.pool.core);
        let (v, export_filter) = {
            let mut runner = self.runner(Some(mod_id_u32), false);
            match runner.run_top_chunk(&chunk) {
                Ok(v) => {
                    runner.pool.release_val(v);
                    (Ok(()), runner.export_list.take())
                }
                Err(e) => (Err(e), None),
            }
        };
        self.pool.work.rewind(&mut self.pool.core, mark);
        if let Err(e) = v {
            for old in std::mem::take(&mut self.module_envs[mod_id_u32 as usize])
                .into_iter()
                .flatten()
            {
                self.pool.release_val(old);
            }
            return Err(with_mod(e));
        }
        let mod_env = &self.module_envs[mod_id_u32 as usize];
        let mut exports: Vec<(&str, RVal)> = Vec::new();
        match &export_filter {
            Some(names) => {
                for sym in names {
                    let Some(val) = self
                        .syms
                        .get(sym)
                        .and_then(|id| mod_env.get(id as usize).copied().flatten())
                    else {
                        for (_, v) in exports {
                            self.pool.release_val(v);
                        }
                        for old in std::mem::take(&mut self.module_envs[mod_id_u32 as usize])
                            .into_iter()
                            .flatten()
                        {
                            self.pool.release_val(old);
                        }
                        return Err(Error::runtime(
                            "expr.module",
                            Span::default(),
                            format!("module '{name}' cannot export unknown symbol '{sym}'"),
                        ));
                    };
                    self.pool.retain_val(val);
                    if let Some(pos) = exports.iter().position(|(k, _)| *k == sym.as_str()) {
                        let prev = std::mem::replace(&mut exports[pos].1, val);
                        self.pool.release_val(prev);
                    } else {
                        exports.push((sym.as_str(), val));
                    }
                }
            }
            None => {
                for (id, val_opt) in mod_env.iter().enumerate() {
                    let Some(val) = *val_opt else { continue };
                    let is_module = matches!(val, RVal::Obj(id) if matches!(self.pool.get(id), Obj::Module { .. }));
                    if !is_module {
                        self.pool.retain_val(val);
                        exports.push((self.syms.names[id].as_ref(), val));
                    }
                }
            }
        }
        exports.sort_by_key(|(a, _)| *a);
        let mod_obj_id = match self.pool.alloc_module_owned(name, &exports, Span::default()) {
            Ok(id) => id,
            Err(e) => {
                for old in std::mem::take(&mut self.module_envs[mod_id_u32 as usize])
                    .into_iter()
                    .flatten()
                {
                    self.pool.release_val(old);
                }
                return Err(with_mod(e));
            }
        };
        if let Some(prev) = self.modules.insert(name.to_string(), RVal::Obj(mod_obj_id)) {
            self.pool.release_val(prev);
        }
        Ok(())
    }

    /// 読み込み済みモジュールの公開関数をソース再解析なしで直接呼び出す。
    pub(super) fn call_in(
        &mut self,
        mod_name: &str,
        fn_name: &str,
        args: &[Value],
        frozen: bool,
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
        let Some(callee) = Pool::find_module_export(*exports, fn_name) else {
            return Err(Error::runtime(
                "expr.module",
                Span::default(),
                available(
                    format!("module '{mod_name}' has no export '{fn_name}'"),
                    "exports",
                    exports.iter().map(|e| e.name.as_str()),
                ),
            ));
        };
        self.call_rval(callee, args, frozen)
    }
}
