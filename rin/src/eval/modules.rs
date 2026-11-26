//! ホスト提供モジュールの読み込みとモジュール内の呼び出し。
use super::*;

impl EnvState<'_> {
    /// ホスト提供のソースから名前付きモジュールを読み込んで評価する。
    pub(super) fn load_module(&mut self, name: &str, source: &str) -> Result<(), Error> {
        let with_mod = |mut err: Error| {
            err.resolve_module(Some(name.to_string()));
            err
        };
        let mod_id = self.modules.ensure_module(&mut self.pool, name).map_err(with_mod)?;
        let mark = self.pool.work.mark(&self.pool.core);
        let chunk = (|| {
            let exprs = parse_program_work(source, &mut self.pool.work, &mut self.pool.core, false)
                .map_err(with_mod)?;
            compile_program(
                exprs.as_slice(),
                Some(mod_id),
                &mut self.syms,
                &mut self.pool,
                &self.globals,
                false,
            )
            .map_err(with_mod)
        })();
        self.pool.work.rewind(&mut self.pool.core, mark);
        let chunk = chunk?;
        self.modules.clear_env(mod_id, &mut self.pool);
        let mark = self.pool.work.mark(&self.pool.core);
        let (v, export_filter) = {
            let mut runner = self.runner(Some(mod_id), false);
            match runner.run_top_chunk(&chunk) {
                Ok(v) => {
                    runner.pool.release_val(v);
                    (Ok(()), runner.export_list.take())
                }
                Err(e) => (Err(e), None),
            }
        };
        if let Err(e) = v {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.modules.clear_env(mod_id, &mut self.pool);
            return Err(with_mod(e));
        }
        // `export_filter` は `WorkArena` 上にあるため、exports 構築が終わってから巻き戻す。
        let mut exports: RawVec<(RawStr, RVal)> = RawVec::new();
        let oom_mod = || {
            with_mod(Error::new(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            ))
        };
        match export_filter {
            Some(names) => {
                for &sym_raw in names.as_slice() {
                    let sym = sym_raw.as_str();
                    let Some(val) = self
                        .syms
                        .get(sym)
                        .and_then(|id| self.modules.env(mod_id).get(id))
                    else {
                        for &(_, v) in exports.as_slice() {
                            self.pool.release_val(v);
                        }
                        self.pool.work.rewind(&mut self.pool.core, mark);
                        self.modules.clear_env(mod_id, &mut self.pool);
                        return Err(Error::runtime(
                            "expr.module",
                            Span::default(),
                            format!("module '{name}' cannot export unknown symbol '{sym}'"),
                        ));
                    };
                    self.pool.retain_val(val);
                    if let Some(pos) = exports.as_slice().iter().position(|(k, _)| k.as_str() == sym)
                    {
                        let prev = std::mem::replace(&mut exports.as_mut_slice()[pos].1, val);
                        self.pool.release_val(prev);
                    } else if exports
                        .push_work(&mut self.pool.work, &mut self.pool.core, (sym_raw, val))
                        .is_none()
                    {
                        self.pool.release_val(val);
                        for &(_, v) in exports.as_slice() {
                            self.pool.release_val(v);
                        }
                        self.pool.work.rewind(&mut self.pool.core, mark);
                        self.modules.clear_env(mod_id, &mut self.pool);
                        return Err(oom_mod());
                    }
                }
            }
            None => {
                let mut defined: RawVec<(u32, RVal)> = RawVec::new();
                let mut ok = true;
                for (id, val) in self.modules.env(mod_id).iter_defined() {
                    if defined
                        .push_work(&mut self.pool.work, &mut self.pool.core, (id, val))
                        .is_none()
                    {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    self.pool.work.rewind(&mut self.pool.core, mark);
                    self.modules.clear_env(mod_id, &mut self.pool);
                    return Err(oom_mod());
                }
                for &(id, val) in defined.as_slice() {
                    let is_module = matches!(val, RVal::Obj(oid) if matches!(self.pool.get(oid), Obj::Module { .. }));
                    if !is_module {
                        let Some(sym_name) = self.syms.name(id) else {
                            continue;
                        };
                        let Some(raw) =
                            RawStr::from_work(&mut self.pool.work, &mut self.pool.core, sym_name)
                        else {
                            for &(_, v) in exports.as_slice() {
                                self.pool.release_val(v);
                            }
                            self.pool.work.rewind(&mut self.pool.core, mark);
                            self.modules.clear_env(mod_id, &mut self.pool);
                            return Err(oom_mod());
                        };
                        self.pool.retain_val(val);
                        if exports
                            .push_work(&mut self.pool.work, &mut self.pool.core, (raw, val))
                            .is_none()
                        {
                            self.pool.release_val(val);
                            for &(_, v) in exports.as_slice() {
                                self.pool.release_val(v);
                            }
                            self.pool.work.rewind(&mut self.pool.core, mark);
                            self.modules.clear_env(mod_id, &mut self.pool);
                            return Err(oom_mod());
                        }
                    }
                }
            }
        }
        exports.as_mut_slice().sort_by(|(a, _), (b, _)| a.as_str().cmp(b.as_str()));
        let mut export_pairs: RawVec<(&str, RVal)> = RawVec::new();
        for (k, v) in exports.as_slice() {
            if export_pairs
                .push_work(&mut self.pool.work, &mut self.pool.core, (k.as_str(), *v))
                .is_none()
            {
                for &(_, val) in exports.as_slice() {
                    self.pool.release_val(val);
                }
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.modules.clear_env(mod_id, &mut self.pool);
                return Err(oom_mod());
            }
        }
        let mod_obj_res =
            self.pool
                .alloc_module_owned(name, export_pairs.as_slice(), Span::default());
        self.pool.work.rewind(&mut self.pool.core, mark);
        let mod_obj_id = match mod_obj_res {
            Ok(id) => id,
            Err(e) => {
                self.modules.clear_env(mod_id, &mut self.pool);
                return Err(with_mod(e));
            }
        };
        if let Some(prev) = self.modules.set_val(mod_id, RVal::Obj(mod_obj_id)) {
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
        let Some(RVal::Obj(mod_id)) = self.modules.get_val(mod_name) else {
            return Err(Error::runtime(
                "expr.module",
                Span::default(),
                suggest(
                    format!("unknown module '{mod_name}'"),
                    mod_name,
                    self.modules.loaded_names(),
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
