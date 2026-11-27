//! `exec_rin_frame` から切り出した複合データ構築・アクセス・モジュール操作命令の実行補助。

use std::fmt::Write;

use super::*;
use crate::mem::RawVec;

impl Runner<'_, '_> {
    pub(super) fn exec_check(&mut self, equality: bool, span: Span) -> Result<(), ErrRecorded> {
        let expected = if equality {
            self.stack.pop().unwrap()
        } else {
            RVal::Bool(true)
        };
        let actual = self.stack.pop().unwrap();
        let result = if equality {
            self.eq_cmp(true, &[actual, expected], span)
        } else {
            Ok(RVal::Bool(matches!(actual, RVal::Bool(true))))
        };
        #[cfg(feature = "std-value")]
        if matches!(result, Ok(RVal::Bool(false))) {
            self.check_failures.push(crate::test::CheckFailure {
                at: span,
                actual: self.pool.export(actual),
                expected: self.pool.export(expected),
                equality,
            });
        }
        self.pool.release_val(actual);
        if equality {
            self.pool.release_val(expected);
        }
        result?;
        self.stack_push(RVal::Void, span)
    }

    pub(super) fn exec_format_template(&mut self, n: u32, span: Span) -> Result<(), ErrRecorded> {
        let count = n as usize;
        let start = self.stack.len() - count;
        let mark = self.pool.work.mark(&self.pool.core);
        let mut buf = RawVec::<u8>::new();

        struct WorkFmt<'a, 'b> {
            buf: &'a mut RawVec<u8>,
            work: &'a mut crate::mem::WorkArena,
            core: &'a mut crate::mem::AllocCore<'b>,
            oom: bool,
        }
        impl std::fmt::Write for WorkFmt<'_, '_> {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                if self.oom {
                    return Err(std::fmt::Error);
                }
                if self
                    .buf
                    .extend_from_slice_work(self.work, self.core, s.as_bytes())
                    .is_none()
                {
                    self.oom = true;
                    return Err(std::fmt::Error);
                }
                Ok(())
            }
        }

        let mut bad_type: Option<&'static str> = None;
        let mut oom = false;
        for i in 0..count {
            let val = self.stack.as_slice()[start + i];
            match val {
                RVal::Int(v) => {
                    let mut wf = WorkFmt {
                        buf: &mut buf,
                        work: &mut self.pool.work,
                        core: &mut self.pool.core,
                        oom: false,
                    };
                    let _ = write!(&mut wf, "{v}");
                    if wf.oom {
                        oom = true;
                        break;
                    }
                }
                RVal::Float(x) => {
                    struct FloatFmt(f64);
                    impl std::fmt::Display for FloatFmt {
                        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                            crate::lexer::write_rin_float(f, self.0)
                        }
                    }
                    let mut wf = WorkFmt {
                        buf: &mut buf,
                        work: &mut self.pool.work,
                        core: &mut self.pool.core,
                        oom: false,
                    };
                    let _ = write!(&mut wf, "{}", FloatFmt(x));
                    if wf.oom {
                        oom = true;
                        break;
                    }
                }
                RVal::Bool(b) => {
                    let s = if b { "true" } else { "false" };
                    if buf
                        .extend_from_slice_work(
                            &mut self.pool.work,
                            &mut self.pool.core,
                            s.as_bytes(),
                        )
                        .is_none()
                    {
                        oom = true;
                        break;
                    }
                }
                RVal::Char(c) => {
                    let mut tmp = [0u8; 4];
                    let bytes = c.encode_utf8(&mut tmp).as_bytes();
                    if buf
                        .extend_from_slice_work(&mut self.pool.work, &mut self.pool.core, bytes)
                        .is_none()
                    {
                        oom = true;
                        break;
                    }
                }
                RVal::Obj(id) => match *self.pool.get(id) {
                    Obj::Str(s) | Obj::Ident(s) => {
                        let bytes = s.as_str().as_bytes();
                        if buf
                            .extend_from_slice_work(&mut self.pool.work, &mut self.pool.core, bytes)
                            .is_none()
                        {
                            oom = true;
                            break;
                        }
                    }
                    _ => {
                        bad_type = Some(val.type_name(self.pool));
                        break;
                    }
                },
                _ => {
                    bad_type = Some(val.type_name(self.pool));
                    break;
                }
            }
        }

        if let Some(tname) = bad_type {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.release_stack_from(start);
            return Err(self.err_runtime_fmt(
                "expr.template",
                span,
                format_args!(
                    "cannot interpolate value of type '{tname}' in template (expected i64, f64, bool, char, text, or ident)"
                ),
            ));
        }
        if oom {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.release_stack_from(start);
            return Err(self.pool.core.err_oom(span));
        }

        let out_bytes = buf.len();
        if !self.try_charge(out_bytes) {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.release_stack_from(start);
            return self.step_limit_result(span);
        }

        // SAFETY: `buf` には有効な UTF-8 バイト列のみ追記している。
        let text = unsafe { std::str::from_utf8_unchecked(buf.as_slice()) };
        let res = self.pool.alloc_str(text, span);
        self.pool.work.rewind(&mut self.pool.core, mark);
        self.release_stack_from(start);
        let obj = res?;
        self.stack_push(RVal::Obj(obj), span)
    }

    pub(super) fn exec_make_list_splice(
        &mut self,
        chunk: &Chunk,
        mask_idx: u32,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        let mask = chunk.splice_masks[mask_idx as usize].as_slice();
        let n = mask.len();
        let start = self.stack.len() - n;
        let mut total_len = 0usize;
        for (i, &is_splice) in mask.iter().enumerate() {
            let val = self.stack.as_slice()[start + i];
            if is_splice != 0 {
                if !self.is_list(val) {
                    let tname = val.type_name(self.pool);
                    self.release_stack_from(start);
                    return Err(self.err_runtime_fmt(
                        "expr.quasiquote",
                        span,
                        format_args!("unquote-splicing ',@' expects list, got {tname}"),
                    ));
                }
                total_len = total_len.saturating_add(self.pool.list_len(val));
            } else {
                total_len = total_len.saturating_add(1);
            }
        }
        if !self.try_charge(total_len) {
            self.release_stack_from(start);
            return self.step_limit_result(span);
        }
        let mut acc = RVal::EmptyList;
        for i in (0..n).rev() {
            let item = self.stack.as_slice()[start + i];
            if mask[i] == 0 {
                match self.pool.alloc_cons(item, acc, span) {
                    Ok(next) => {
                        self.pool.release_val(acc);
                        acc = next;
                    }
                    Err(e) => {
                        self.pool.release_val(acc);
                        self.release_stack_from(start);
                        return Err(e);
                    }
                }
                self.pool.release_val(item);
                self.stack.pop();
            } else if matches!(acc, RVal::EmptyList) {
                acc = item;
                self.stack.pop();
            } else if matches!(item, RVal::EmptyList) {
                self.stack.pop();
            } else {
                let mark = self.pool.work.mark(&self.pool.core);
                let mut elems = RawVec::<RVal>::new();
                let mut cur = item;
                let mut oom = false;
                while let RVal::Obj(id) = cur {
                    let Obj::Cons { head, tail, .. } = *self.pool.get(id) else {
                        break;
                    };
                    if elems
                        .push_work(&mut self.pool.work, &mut self.pool.core, head)
                        .is_none()
                    {
                        oom = true;
                        break;
                    }
                    cur = tail;
                }
                if oom {
                    self.pool.work.rewind(&mut self.pool.core, mark);
                    self.pool.release_val(acc);
                    self.release_stack_from(start);
                    return Err(self.work_oom(span));
                }
                for &elem in elems.as_slice().iter().rev() {
                    match self.pool.alloc_cons(elem, acc, span) {
                        Ok(next) => {
                            self.pool.release_val(acc);
                            acc = next;
                        }
                        Err(e) => {
                            self.pool.work.rewind(&mut self.pool.core, mark);
                            self.pool.release_val(acc);
                            self.release_stack_from(start);
                            return Err(e);
                        }
                    }
                }
                self.pool.work.rewind(&mut self.pool.core, mark);
                self.pool.release_val(item);
                self.stack.pop();
            }
        }
        self.stack_push(acc, span)
    }

    pub(super) fn exec_make_tuple(&mut self, n: u32, span: Span) -> Result<(), ErrRecorded> {
        let start = self.stack.len() - n as usize;
        let mark = self.pool.work.mark(&self.pool.core);
        let mut tmp = RawVec::<RVal>::new();
        if tmp
            .reserve_work(&mut self.pool.work, &mut self.pool.core, n as usize)
            .is_none()
        {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.release_stack_from(start);
            return Err(self.work_oom(span));
        }
        let _ = tmp.extend_from_slice_work(
            &mut self.pool.work,
            &mut self.pool.core,
            &self.stack.as_slice()[start..],
        );
        self.stack.truncate(start);
        let id = self.pool.alloc_tuple_owned(tmp.as_slice(), span);
        self.pool.work.rewind(&mut self.pool.core, mark);
        self.stack_push(RVal::Obj(id?), span)
    }

    pub(super) fn exec_make_struct(
        &mut self,
        chunk: &Chunk,
        s_idx: u32,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        let desc = &chunk.structs[s_idx as usize];
        let n = desc.fields.len();
        let start = self.stack.len() - n;
        let mark = self.pool.work.mark(&self.pool.core);
        let mut pairs = RawVec::<(&str, RVal)>::new();
        if pairs
            .reserve_work(&mut self.pool.work, &mut self.pool.core, n)
            .is_none()
        {
            self.pool.work.rewind(&mut self.pool.core, mark);
            self.release_stack_from(start);
            return Err(self.work_oom(span));
        }
        for (idx, fname) in desc.fields.as_slice().iter().enumerate() {
            let val = self.stack.as_slice()[start + idx];
            let _ = pairs.push_work(
                &mut self.pool.work,
                &mut self.pool.core,
                (fname.as_ref(), val),
            );
        }
        self.stack.truncate(start);
        // 安定マージソート（O(F log F)、システムヒープ不使用）
        if super::super::sort_field_pairs_work(
            pairs.as_mut_slice(),
            &mut self.pool.work,
            &mut self.pool.core,
        )
        .is_none()
        {
            for &(_, v) in pairs.as_slice() {
                self.pool.release_val(v);
            }
            self.pool.work.rewind(&mut self.pool.core, mark);
            return Err(self.work_oom(span));
        }
        // ソート済み配列を 1 パスで重複排除（後ろ勝ち・旧値は解放）
        let mut field_names = RawVec::<&str>::new();
        let mut field_vals = RawVec::<RVal>::new();
        if field_names
            .reserve_work(&mut self.pool.work, &mut self.pool.core, pairs.len())
            .is_none()
            || field_vals
                .reserve_work(&mut self.pool.work, &mut self.pool.core, pairs.len())
                .is_none()
        {
            for &(_, v) in pairs.as_slice() {
                self.pool.release_val(v);
            }
            self.pool.work.rewind(&mut self.pool.core, mark);
            return Err(self.work_oom(span));
        }
        for &(k, v) in pairs.as_slice() {
            if field_names.last().copied() == Some(k) {
                let prev = std::mem::replace(field_vals.last_mut().unwrap(), v);
                self.pool.release_val(prev);
            } else {
                let _ = field_names.push_work(&mut self.pool.work, &mut self.pool.core, k);
                let _ = field_vals.push_work(&mut self.pool.work, &mut self.pool.core, v);
            }
        }
        let desc_ref =
            match self
                .pool
                .intern_struct_desc(desc.name.as_ref(), field_names.as_slice(), span)
            {
                Ok(d) => d,
                Err(e) => {
                    self.release_slice(field_vals.as_slice());
                    self.pool.work.rewind(&mut self.pool.core, mark);
                    return Err(e);
                }
            };
        let id = self
            .pool
            .alloc_struct_owned(desc_ref, field_vals.as_slice(), span);
        self.pool.work.rewind(&mut self.pool.core, mark);
        self.stack_push(RVal::Obj(id?), span)
    }

    pub(super) fn exec_with_check_struct(&mut self, span: Span) -> Result<(), ErrRecorded> {
        let base_val = *self
            .stack
            .last()
            .expect("stack underflow on WithCheckStruct");
        let is_struct = matches!(
            base_val,
            RVal::Obj(id) if matches!(self.pool.get(id), Obj::Struct { .. })
        );
        if !is_struct {
            let tname = base_val.type_name(self.pool);
            return Err(self.err_runtime_fmt(
                "expr.with",
                span,
                format_args!("'with' requires a struct, got {tname}"),
            ));
        }
        Ok(())
    }

    pub(super) fn exec_with_check_field(
        &mut self,
        chunk: &Chunk,
        f_idx: u32,
        pushed_count: u32,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        let fname = &chunk.strings[f_idx as usize];
        let base_val = self.stack.as_slice()[self.stack.len() - 1 - pushed_count as usize];
        let RVal::Obj(id) = base_val else {
            unreachable!();
        };
        let Obj::Struct { desc, .. } = *self.pool.get(id) else {
            unreachable!();
        };
        if self
            .pool
            .struct_desc_find_field(desc, fname.as_ref())
            .is_none()
        {
            return Err(self.pool.err_struct_missing_field(
                "expr.with",
                span,
                desc,
                fname.as_ref(),
            ));
        }
        Ok(())
    }

    pub(super) fn exec_with_finish(
        &mut self,
        chunk: &Chunk,
        s_idx: u32,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        let patch_desc = &chunk.structs[s_idx as usize];
        let n = patch_desc.fields.len();
        let val_start = self.stack.len() - n;
        let base_val = self.stack.as_slice()[val_start - 1];
        let RVal::Obj(id) = base_val else {
            unreachable!();
        };
        let Obj::Struct { desc, values } = *self.pool.get(id) else {
            unreachable!();
        };
        if !super::super::runner::try_charge_steps(&mut self.steps_left, values.len()) {
            return self.step_limit_result(span);
        }
        let mark = self.pool.work.mark(&self.pool.core);
        let mut updated = RawVec::<RVal>::new();
        if updated
            .extend_from_slice_work(&mut self.pool.work, &mut self.pool.core, values.as_slice())
            .is_none()
        {
            self.pool.work.rewind(&mut self.pool.core, mark);
            return Err(self.work_oom(span));
        }
        for &v in updated.as_slice() {
            self.pool.retain_val(v);
        }
        for (idx, fname) in patch_desc.fields.as_slice().iter().enumerate() {
            let val = self.stack.as_slice()[val_start + idx];
            let fpos = self
                .pool
                .struct_desc_find_field(desc, fname.as_ref())
                .expect("WithCheckField already verified field");
            let prev = std::mem::replace(&mut updated.as_mut_slice()[fpos], val);
            self.pool.release_val(prev);
        }
        self.stack.truncate(val_start - 1);
        self.pool.struct_desc_retain(desc);
        self.pool.release_val(base_val);
        let new_id = self.pool.alloc_struct_owned(desc, updated.as_slice(), span);
        self.pool.work.rewind(&mut self.pool.core, mark);
        self.stack_push(RVal::Obj(new_id?), span)
    }

    pub(super) fn exec_get_field(
        &mut self,
        chunk: &Chunk,
        f_idx: u32,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        let field = &chunk.strings[f_idx as usize];
        let val = self.stack.pop().expect("stack underflow on GetField");
        let res = match val {
            RVal::Obj(id) => match *self.pool.get(id) {
                Obj::Struct { desc, values } => {
                    match self.pool.struct_desc_find_field(desc, field.as_ref()) {
                        Some(fpos) => {
                            let fv = values[fpos];
                            self.pool.retain_val(fv);
                            Ok(fv)
                        }
                        None => Err(self.pool.err_struct_missing_field(
                            "expr.field",
                            span,
                            desc,
                            field.as_ref(),
                        )),
                    }
                }
                Obj::Module { name, exports } => {
                    match crate::pool::Pool::find_module_export(exports, field.as_ref()) {
                        Some(fv) => {
                            self.pool.retain_val(fv);
                            Ok(fv)
                        }
                        None => {
                            let out = self.pool.core.error.begin_msg(
                                "expr.module",
                                ErrorKind::Runtime,
                                span,
                            );
                            let _ =
                                write!(out, "module '{}' has no export '{field}'", name.as_str());
                            write_available(
                                out,
                                "exports",
                                exports.iter().map(|e| e.name.as_str()),
                            );
                            Err(ErrRecorded)
                        }
                    }
                }
                _ => {
                    let tname = val.type_name(self.pool);
                    Err(self.err_runtime_fmt(
                        "expr.field",
                        span,
                        format_args!("cannot read field '{field}' on {tname}"),
                    ))
                }
            },
            other => {
                let tname = other.type_name(self.pool);
                Err(self.err_runtime_fmt(
                    "expr.field",
                    span,
                    format_args!("cannot read field '{field}' on {tname}"),
                ))
            }
        };
        self.pool.release_val(val);
        self.stack_push(res?, span)
    }

    pub(super) fn exec_get_index(&mut self, span: Span) -> Result<(), ErrRecorded> {
        let idx_val = self.stack.pop().expect("stack underflow on GetIndex");
        let list_val = self.stack.pop().expect("stack underflow on GetIndex");
        let idx = match idx_val {
            RVal::Int(i) => i,
            other => {
                let tname = other.type_name(self.pool);
                self.pool.release_val(idx_val);
                self.pool.release_val(list_val);
                return Err(self.err_runtime_fmt(
                    "expr.index",
                    span,
                    format_args!("index must be i64, got {tname}"),
                ));
            }
        };
        self.pool.release_val(idx_val);

        let res = if let RVal::Obj(id) = list_val
            && let Obj::Array(arr) = *self.pool.get(id)
        {
            if idx < 0 || (idx as usize) >= arr.len() {
                Err(self.err_runtime_fmt(
                    "expr.index",
                    span,
                    format_args!("index {idx} out of range (length {})", arr.len()),
                ))
            } else {
                let v = arr[idx as usize];
                self.pool.retain_val(v);
                Ok(v)
            }
        } else if let RVal::Obj(id) = list_val
            && let Obj::Str(s) = *self.pool.get(id)
        {
            if idx < 0 || (idx as usize) >= s.char_len() {
                Err(self.err_runtime_fmt(
                    "expr.index",
                    span,
                    format_args!("index {idx} out of range (length {})", s.char_len()),
                ))
            } else {
                let idx_u = idx as usize;
                if !s.is_ascii() && !self.try_charge(idx_u + 1) {
                    self.pool.release_val(list_val);
                    return self.step_limit_result(span);
                }
                let (ch, _) = s.char_at(idx_u).unwrap();
                Ok(RVal::Char(ch))
            }
        } else if !self.is_list(list_val) {
            let tname = list_val.type_name(self.pool);
            Err(self.err_runtime_fmt(
                "expr.index",
                span,
                format_args!("cannot index into {tname}"),
            ))
        } else if idx < 0 {
            let len = self.pool.list_len(list_val);
            Err(self.err_runtime_fmt(
                "expr.index",
                span,
                format_args!("index {idx} out of range (length {len})"),
            ))
        } else {
            match self.list_get(list_val, idx as usize, span) {
                Err(e) => Err(e),
                Ok(Some(v)) => {
                    self.pool.retain_val(v);
                    Ok(v)
                }
                Ok(None) => {
                    let len = self.pool.list_len(list_val);
                    Err(self.err_runtime_fmt(
                        "expr.index",
                        span,
                        format_args!("index {idx} out of range (length {len})"),
                    ))
                }
            }
        };
        self.pool.release_val(list_val);
        self.stack_push(res?, span)
    }

    pub(super) fn exec_import(&mut self, id: u32, span: Span) -> Result<(), ErrRecorded> {
        if self.frozen {
            return Err(self.err_runtime(
                "env.frozen",
                span,
                "'import' is not allowed in a frozen environment",
            ));
        }
        let name = self.syms.name(id).unwrap_or("?");
        let Some(mod_val) = self.modules.get_val(name) else {
            let out = self
                .pool
                .core
                .error
                .begin_msg("expr.module", ErrorKind::Runtime, span);
            let _ = write!(out, "unknown module '{name}'");
            write_suggest(out, name, self.modules.loaded_names());
            return Err(ErrRecorded);
        };
        self.pool.retain_val(mod_val);
        self.define_top(id, mod_val, span)?;
        self.stack_push(RVal::Void, span)
    }

    pub(super) fn exec_export(
        &mut self,
        chunk: &Chunk,
        idx: u32,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        if self.frozen {
            return Err(self.err_runtime(
                "env.frozen",
                span,
                "'export' is not allowed in a frozen environment",
            ));
        }
        if self.active_module.is_none() {
            return Err(self.err_runtime(
                "expr.module",
                span,
                "'export' can only be used inside a module",
            ));
        }
        let names = &chunk.export_lists[idx as usize];
        let list = self.export_list.get_or_insert_with(RawVec::new);
        for &sym in names.as_slice() {
            let Some(raw) = crate::mem::RawStr::from_work(
                &mut self.pool.work,
                &mut self.pool.core,
                sym.as_str(),
            ) else {
                return Err(self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    span,
                    "memory limit exceeded",
                ));
            };
            if list
                .push_work(&mut self.pool.work, &mut self.pool.core, raw)
                .is_none()
            {
                return Err(self.pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    span,
                    "memory limit exceeded",
                ));
            }
        }
        self.stack_push(RVal::Void, span)
    }
}
