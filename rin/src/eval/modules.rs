//! ホスト提供モジュールの読み込みとモジュール内の呼び出し。
#[cfg(feature = "std-value")]
use std::fmt::Write;

use super::*;

impl EnvState<'_> {
    /// ホスト提供のソースから名前付きモジュールを読み込んで評価する（所有エラー返却）。
    #[cfg(feature = "std-value")]
    pub(super) fn load_module(&mut self, name: &str, source: &str) -> Result<(), Error> {
        self.load_module_inner(name, source)
            .map_err(|_| self.pool.core.error.to_owned_error())
    }

    /// ホスト提供のソースから名前付きモジュールを読み込んで評価する（システムヒープ不使用）。
    pub(super) fn load_module_inner(
        &mut self,
        name: &str,
        source: &str,
    ) -> Result<(), ErrRecorded> {
        if self.modules.get_val(name).is_some() {
            return Err(self.pool.core.err_fmt(
                "expr.module",
                ErrorKind::Runtime,
                Span::default(),
                format_args!("module '{name}' is already loaded"),
            ));
        }
        if let Some(sym_id) = self.syms.get(name)
            && (sym_id < STATIC_SYM_COUNT || self.globals.get(sym_id).is_some())
        {
            return Err(self.pool.core.err_fmt(
                "expr.module",
                ErrorKind::Runtime,
                Span::default(),
                format_args!("module name '{name}' conflicts with existing binding"),
            ));
        }
        let mod_id = match self.modules.ensure_module(&mut self.pool, name) {
            Ok(id) => id,
            Err(e) => {
                self.pool.core.error.resolve_module(Some(name));
                return Err(e);
            }
        };
        let mark = self.pool.work.mark(&self.pool.core);
        let chunk = match (|| {
            let exprs =
                parse_program_work(source, &mut self.pool.work, &mut self.pool.core, false)?;
            compile_program(
                exprs.as_slice(),
                Some(mod_id),
                &mut self.syms,
                &mut self.pool,
                &self.globals,
                false,
            )
        })() {
            Ok(chunk) => chunk,
            Err(e) => {
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.core.error.resolve_module(Some(name));
                return Err(e);
            }
        };
        self.modules.clear_env(mod_id, &mut self.pool);
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
            self.pool.core.error.resolve_module(Some(name));
            return Err(e);
        }
        // `export_filter` は `WorkArena` 上にあるため、exports 構築が終わってから巻き戻す。
        let mut raw_exports: RawVec<(RawStr, RVal)> = RawVec::new();
        match export_filter {
            Some(names) => {
                for &sym_raw in names.as_slice() {
                    let sym = sym_raw.as_str();
                    let Some(val) = self
                        .syms
                        .get(sym)
                        .and_then(|id| self.modules.env(mod_id).get(id))
                    else {
                        let mut sym_buf = crate::error::InlineBuf::<256>::new();
                        sym_buf.push_str(sym);
                        self.pool.work.rewind(&mut self.pool.core, mark);
                        self.modules.clear_env(mod_id, &mut self.pool);
                        return Err(self.pool.core.err_fmt(
                            "expr.module",
                            ErrorKind::Runtime,
                            Span::default(),
                            format_args!(
                                "module '{name}' cannot export unknown symbol '{}'",
                                sym_buf.as_str()
                            ),
                        ));
                    };
                    if raw_exports
                        .push_work(&mut self.pool.work, &mut self.pool.core, (sym_raw, val))
                        .is_none()
                    {
                        self.pool.work.rewind(&mut self.pool.core, mark);
                        self.modules.clear_env(mod_id, &mut self.pool);
                        let e = self.pool.core.err_str(
                            "eval.limits",
                            ErrorKind::TooMuchMemory,
                            Span::default(),
                            "memory limit exceeded",
                        );
                        self.pool.core.error.resolve_module(Some(name));
                        return Err(e);
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
                    let e = self.pool.core.err_str(
                        "eval.limits",
                        ErrorKind::TooMuchMemory,
                        Span::default(),
                        "memory limit exceeded",
                    );
                    self.pool.core.error.resolve_module(Some(name));
                    return Err(e);
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
                            self.pool.work.rewind(&mut self.pool.core, mark);
                            self.modules.clear_env(mod_id, &mut self.pool);
                            let e = self.pool.core.err_str(
                                "eval.limits",
                                ErrorKind::TooMuchMemory,
                                Span::default(),
                                "memory limit exceeded",
                            );
                            self.pool.core.error.resolve_module(Some(name));
                            return Err(e);
                        };
                        if raw_exports
                            .push_work(&mut self.pool.work, &mut self.pool.core, (raw, val))
                            .is_none()
                        {
                            self.pool.work.rewind(&mut self.pool.core, mark);
                            self.modules.clear_env(mod_id, &mut self.pool);
                            let e = self.pool.core.err_str(
                                "eval.limits",
                                ErrorKind::TooMuchMemory,
                                Span::default(),
                                "memory limit exceeded",
                            );
                            self.pool.core.error.resolve_module(Some(name));
                            return Err(e);
                        }
                    }
                }
            }
        }
        let mut export_pairs: RawVec<(&str, RVal)> = RawVec::new();
        if export_pairs
            .reserve_work(&mut self.pool.work, &mut self.pool.core, raw_exports.len())
            .is_none()
        {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.modules.clear_env(mod_id, &mut self.pool);
            let e = self.pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            self.pool.core.error.resolve_module(Some(name));
            return Err(e);
        }
        for (k, v) in raw_exports.as_slice() {
            let _ =
                export_pairs.push_work(&mut self.pool.work, &mut self.pool.core, (k.as_str(), *v));
        }
        if sort_field_pairs_work(
            export_pairs.as_mut_slice(),
            &mut self.pool.work,
            &mut self.pool.core,
        )
        .is_none()
        {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.modules.clear_env(mod_id, &mut self.pool);
            let e = self.pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            self.pool.core.error.resolve_module(Some(name));
            return Err(e);
        }
        // ソート済みの公開一覧から重複を 1 パスで除去し、各公開要素につき参照を 1 つだけ保持する。
        let mut unique_len = 0usize;
        {
            let slice = export_pairs.as_mut_slice();
            for i in 0..slice.len() {
                let (k, v) = slice[i];
                if unique_len > 0 && slice[unique_len - 1].0 == k {
                    slice[unique_len - 1].1 = v;
                } else {
                    slice[unique_len] = (k, v);
                    unique_len += 1;
                }
            }
        }
        export_pairs.truncate(unique_len);
        for &(_, v) in export_pairs.as_slice() {
            self.pool.retain_val(v);
        }
        let mod_obj_res =
            self.pool
                .alloc_module_owned(name, export_pairs.as_slice(), Span::default());
        self.pool.work.rewind(&mut self.pool.core, mark);
        let mod_obj_id = match mod_obj_res {
            Ok(id) => id,
            Err(e) => {
                self.modules.clear_env(mod_id, &mut self.pool);
                self.pool.core.error.resolve_module(Some(name));
                return Err(e);
            }
        };
        if let Some(prev) = self.modules.set_val(mod_id, RVal::Obj(mod_obj_id)) {
            self.pool.release_val(prev);
        }
        Ok(())
    }

    /// 読み込み済みモジュールの公開関数をソース再解析なしで直接呼び出す。
    #[cfg(feature = "std-value")]
    pub(super) fn call_in(
        &mut self,
        mod_name: &str,
        fn_name: &str,
        args: &[Value],
        frozen: bool,
    ) -> Result<Value, Error> {
        let Some(RVal::Obj(mod_id)) = self.modules.get_val(mod_name) else {
            let out =
                self.pool
                    .core
                    .error
                    .begin_msg("expr.module", ErrorKind::Runtime, Span::default());
            let _ = write!(out, "unknown module '{mod_name}'");
            write_suggest(out, mod_name, self.modules.loaded_names());
            return Err(self.pool.core.error.to_owned_error());
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
            write_available(out, "exports", exports.iter().map(|e| e.name.as_str()));
            return Err(self.pool.core.error.to_owned_error());
        };
        self.call_rval(callee, args, frozen)
    }
}
