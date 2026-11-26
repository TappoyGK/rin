use std::fmt::Write;

use super::*;
use crate::mem::RawVec;

impl Runner<'_, '_> {
    pub(super) fn exec_rin_frame<'f>(
        &mut self,
        frame: &mut RinFrame<'f>,
        frames: &mut RawVec<FrameEntry<'f>>,
    ) -> Result<FrameAction, ErrRecorded> {
        let mut ip = frame.ip as usize;
        let mut frame_stack_start = frame.frame_stack_start as usize;
        let mut base = frame.base as usize;
        'frame: loop {
            let chunk = frame.tail_source.as_ref().unwrap_or(&frame.entry).chunk();
            while ip < chunk.code.len() {
                let op = chunk.code[ip];
                ip += 1;

                match op {
                    Op::Tick(start, count) => {
                        self.tick(chunk, start, count)?;
                    }
                    Op::Check(equality) => {
                        let span = chunk.spans[ip - 1];
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
                        self.stack_push(RVal::Void, span)?;
                    }
                    Op::Void => self.stack_push_at(RVal::Void, chunk, ip - 1)?,
                    Op::Int(n) => self.stack_push_at(RVal::Int(n), chunk, ip - 1)?,
                    Op::Float(idx) => {
                        self.stack_push_at(RVal::Float(chunk.floats[idx as usize]), chunk, ip - 1)?
                    }
                    Op::Bool(b) => self.stack_push_at(RVal::Bool(b), chunk, ip - 1)?,
                    Op::Char(c) => self.stack_push_at(RVal::Char(c), chunk, ip - 1)?,
                    Op::Str(idx) => {
                        let s = &chunk.strings[idx as usize];
                        let span = chunk.spans[ip - 1];
                        let id = self.pool.alloc_str(s.as_ref(), span)?;
                        self.stack_push(RVal::Obj(id), span)?;
                    }
                    Op::None => self.stack_push_at(RVal::None, chunk, ip - 1)?,
                    Op::EmptyList => self.stack_push_at(RVal::EmptyList, chunk, ip - 1)?,
                    Op::GetLocal(slot) => {
                        let v = self.stack.as_slice()[base + slot as usize];
                        self.pool.retain_val(v);
                        self.stack_push_at(v, chunk, ip - 1)?;
                    }
                    Op::SetLocal(slot) => {
                        let v = self.stack.pop().expect("stack underflow on SetLocal");
                        let old = std::mem::replace(
                            &mut self.stack.as_mut_slice()[base + slot as usize],
                            v,
                        );
                        self.pool.release_val(old);
                    }
                    Op::GetCapture(idx) => {
                        let v = frame.capture_at(idx as usize);
                        self.pool.retain_val(v);
                        self.stack_push_at(v, chunk, ip - 1)?;
                    }
                    Op::GetGlobal(id) => {
                        let v = self.lookup_global(id, chunk, ip - 1)?;
                        self.stack_push_at(v, chunk, ip - 1)?;
                    }
                    Op::DefGlobal(id) => {
                        let span = chunk.spans[ip - 1];
                        let v = self.stack.pop().expect("stack underflow on DefGlobal");
                        if self.frozen {
                            self.pool.release_val(v);
                            return Err(self.err_runtime(
                                "env.frozen",
                                span,
                                "'def' is not allowed in a frozen environment",
                            ));
                        }
                        self.define_top(id, v, span)?;
                        self.stack_push(RVal::Void, span)?;
                    }
                    Op::DefFn(name_id, proto_idx) => {
                        let span = chunk.spans[ip - 1];
                        if self.frozen {
                            return Err(self.err_runtime(
                                "env.frozen",
                                span,
                                "'defn' is not allowed in a frozen environment",
                            ));
                        }
                        let proto = chunk.protos[proto_idx as usize];
                        let closure = self.build_closure(proto, frame.captures, base, span)?;
                        self.define_top(name_id, closure, span)?;
                        self.stack_push(RVal::Void, span)?;
                    }
                    Op::MakeClosure(proto_idx) => {
                        let span = chunk.spans[ip - 1];
                        let proto = chunk.protos[proto_idx as usize];
                        let closure = self.build_closure(proto, frame.captures, base, span)?;
                        self.stack_push(closure, span)?;
                    }
                    Op::MakeList(n) => {
                        let span = chunk.spans[ip - 1];
                        let start = self.stack.len() - n as usize;
                        let list = self.build_list_from_owned_slice(start, span)?;
                        self.stack_push(list, span)?;
                    }
                    Op::MakeTuple(n) => {
                        let span = chunk.spans[ip - 1];
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
                        self.stack_push(RVal::Obj(id?), span)?;
                    }
                    Op::MakeStruct(s_idx) => {
                        let span = chunk.spans[ip - 1];
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
                            if let Some(pos) = pairs
                                .as_slice()
                                .iter()
                                .position(|(k, _)| *k == fname.as_ref())
                            {
                                let prev = std::mem::replace(&mut pairs.as_mut_slice()[pos].1, val);
                                self.pool.release_val(prev);
                            } else {
                                let _ = pairs.push_work(
                                    &mut self.pool.work,
                                    &mut self.pool.core,
                                    (fname.as_ref(), val),
                                );
                            }
                        }
                        self.stack.truncate(start);
                        // 挿入ソート（フィールド数は通常少数、システムヒープ不使用）
                        let ps = pairs.as_mut_slice();
                        for i in 1..ps.len() {
                            let mut j = i;
                            while j > 0 && ps[j - 1].0 > ps[j].0 {
                                ps.swap(j - 1, j);
                                j -= 1;
                            }
                        }
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
                            let _ =
                                field_names.push_work(&mut self.pool.work, &mut self.pool.core, k);
                            let _ =
                                field_vals.push_work(&mut self.pool.work, &mut self.pool.core, v);
                        }
                        let desc_ref = match self.pool.intern_struct_desc(
                            desc.name.as_ref(),
                            field_names.as_slice(),
                            span,
                        ) {
                            Ok(d) => d,
                            Err(e) => {
                                self.release_slice(field_vals.as_slice());
                                self.pool.work.rewind(&mut self.pool.core, mark);
                                return Err(e);
                            }
                        };
                        let id =
                            self.pool
                                .alloc_struct_owned(desc_ref, field_vals.as_slice(), span);
                        self.pool.work.rewind(&mut self.pool.core, mark);
                        self.stack_push(RVal::Obj(id?), span)?;
                    }
                    Op::WithCheckStruct => {
                        let span = chunk.spans[ip - 1];
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
                    }
                    Op::WithCheckField(f_idx, pushed_count) => {
                        let span = chunk.spans[ip - 1];
                        let fname = &chunk.strings[f_idx as usize];
                        let base_val =
                            self.stack.as_slice()[self.stack.len() - 1 - pushed_count as usize];
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
                    }
                    Op::WithFinish(s_idx) => {
                        let span = chunk.spans[ip - 1];
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
                        if !super::super::runner::try_charge_steps(
                            &mut self.steps_left,
                            values.len(),
                        ) {
                            return self.step_limit_result(span);
                        }
                        let mark = self.pool.work.mark(&self.pool.core);
                        let mut updated = RawVec::<RVal>::new();
                        if updated
                            .extend_from_slice_work(
                                &mut self.pool.work,
                                &mut self.pool.core,
                                values.as_slice(),
                            )
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
                        self.stack_push(RVal::Obj(new_id?), span)?;
                    }
                    Op::MakeVariant {
                        type_name,
                        variant,
                        has_data,
                    } => {
                        let span = chunk.spans[ip - 1];
                        let data = if has_data {
                            Some(self.stack.pop().expect("stack underflow on MakeVariant"))
                        } else {
                            None
                        };
                        let id = self.pool.alloc_variant_owned(
                            chunk.strings[type_name as usize].as_ref(),
                            chunk.strings[variant as usize].as_ref(),
                            data,
                            span,
                        )?;
                        self.stack_push(RVal::Obj(id), span)?;
                    }
                    Op::MakeSome => {
                        let span = chunk.spans[ip - 1];
                        let v = self.stack.pop().expect("stack underflow on MakeSome");
                        let id = self.pool.alloc_obj(Obj::Some(v), span)?;
                        self.stack_push(RVal::Obj(id), span)?;
                    }
                    Op::MakeOk => {
                        let span = chunk.spans[ip - 1];
                        let v = self.stack.pop().expect("stack underflow on MakeOk");
                        let id = self.pool.alloc_obj(Obj::Ok(v), span)?;
                        self.stack_push(RVal::Obj(id), span)?;
                    }
                    Op::MakeErr => {
                        let span = chunk.spans[ip - 1];
                        let v = self.stack.pop().expect("stack underflow on MakeErr");
                        let id = self.pool.alloc_obj(Obj::Err(v), span)?;
                        self.stack_push(RVal::Obj(id), span)?;
                    }
                    Op::GetField(f_idx) => {
                        let span = chunk.spans[ip - 1];
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
                                    match crate::pool::Pool::find_module_export(
                                        exports,
                                        field.as_ref(),
                                    ) {
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
                                            let _ = write!(
                                                out,
                                                "module '{}' has no export '{field}'",
                                                name.as_str()
                                            );
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
                        self.stack_push(res?, span)?;
                    }
                    Op::GetIndex => {
                        let span = chunk.spans[ip - 1];
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
                        self.stack_push(res?, span)?;
                    }
                    Op::Call(argc) => {
                        let argc = argc as usize;
                        let arg_start = self.stack.len() - argc;
                        let callee = self.stack.as_slice()[arg_start - 1];
                        if let RVal::Builtin(b) = callee {
                            if b.has_callback() {
                                frame.ip = ip as u32;
                                frame.frame_stack_start = frame_stack_start as u32;
                                return Ok(FrameAction::CallCallbackBuiltin {
                                    builtin: b,
                                    arg_start,
                                    argc,
                                    span: chunk.spans[ip - 1],
                                });
                            }
                            self.call_builtin_from_stack(b, arg_start, argc, chunk, ip - 1)?;
                        } else {
                            let span = chunk.spans[ip - 1];
                            let RVal::Obj(id) = callee else {
                                return Err(self.call_type_error(callee, span));
                            };
                            let (proto, captures) = match self.pool.get(id) {
                                Obj::Closure { proto, captures } => (*proto, *captures),
                                _ => return Err(self.call_type_error(callee, span)),
                            };
                            if proto.arity as usize != argc {
                                return Err(self.call_arity_error(proto.arity, argc, span));
                            }
                            self.enter_depth(span)?;
                            let child_base = arg_start;
                            let child_locals = proto.local_count as usize;
                            if child_locals > argc
                                && let Err(e) =
                                    self.stack_resize_void(child_base + child_locals, span)
                            {
                                self.leave_depth();
                                return Err(e);
                            }
                            if frames
                                .reserve_work(&mut self.pool.work, &mut self.pool.core, 1)
                                .is_none()
                            {
                                self.leave_depth();
                                self.release_stack_from(arg_start - 1);
                                return Err(self.work_oom(span));
                            }
                            let prev_mod = std::mem::replace(&mut self.active_module, proto.module);
                            frame.ip = ip as u32;
                            frame.frame_stack_start = frame_stack_start as u32;
                            let _ = frames.push_work(
                                &mut self.pool.work,
                                &mut self.pool.core,
                                FrameEntry::Rin(*frame),
                            );
                            *frame = RinFrame::new(
                                FrameSource::Proto(proto),
                                captures,
                                child_base,
                                child_base + child_locals,
                                Some(CallOrigin::Rin(span)),
                                prev_mod,
                                prev_mod,
                                arg_start - 1,
                                true,
                            );
                            base = child_base;
                            frame_stack_start = child_base + child_locals;
                            ip = 0;
                            continue 'frame;
                        }
                    }
                    Op::TailCall(argc) => {
                        let span = chunk.spans[ip - 1];
                        let argc = argc as usize;
                        let arg_start = self.stack.len() - argc;
                        let callee = self.stack.as_slice()[arg_start - 1];
                        match callee {
                            RVal::Obj(id)
                                if matches!(
                                    frame.tail_source.as_ref().unwrap_or(&frame.entry),
                                    FrameSource::Proto(_)
                                ) =>
                            {
                                let (proto, cap) = match self.pool.get(id) {
                                    Obj::Closure { proto, captures } => (*proto, *captures),
                                    _ => return Err(self.call_type_error(callee, span)),
                                };
                                if proto.arity as usize != argc {
                                    return Err(self.call_arity_error(proto.arity, argc, span));
                                }
                                for i in base..(arg_start - 1) {
                                    let old = self.stack.as_slice()[i];
                                    self.pool.release_val(old);
                                }
                                for i in 0..argc {
                                    let arg = self.stack.as_slice()[arg_start + i];
                                    self.stack.as_mut_slice()[base + i] = arg;
                                }
                                self.stack.truncate(base + argc);
                                let new_locals = proto.local_count as usize;
                                self.stack_resize_void(base + new_locals, span)?;
                                if cap.is_empty() {
                                    self.pool.release_val(callee);
                                } else {
                                    self.stack_push(callee, span)?;
                                }
                                self.active_module = proto.module;
                                frame.tail_source = Some(FrameSource::Proto(proto));
                                frame.tail_calls += 1;
                                frame.captures = cap.as_slice().as_ptr();
                                frame_stack_start = self.stack.len();
                                ip = 0;
                                continue 'frame;
                            }
                            RVal::Builtin(b) => {
                                if b.has_callback() {
                                    frame.ip = ip as u32;
                                    frame.frame_stack_start = frame_stack_start as u32;
                                    return Ok(FrameAction::CallCallbackBuiltin {
                                        builtin: b,
                                        arg_start,
                                        argc,
                                        span,
                                    });
                                }
                                self.call_builtin_from_stack(b, arg_start, argc, chunk, ip - 1)?;
                            }
                            _ => return Err(self.call_type_error(callee, span)),
                        }
                    }
                    Op::And => {
                        let span = chunk.spans[ip - 1];
                        return Err(self.err_runtime(
                            "op.logic",
                            span,
                            "'and' requires at least 1 argument",
                        ));
                    }
                    Op::Or => {
                        let span = chunk.spans[ip - 1];
                        return Err(self.err_runtime(
                            "op.logic",
                            span,
                            "'or' requires at least 1 argument",
                        ));
                    }
                    Op::Jump(target) => {
                        ip = target as usize;
                    }
                    Op::JumpIfFalse(target) => {
                        let cond = self.stack.pop().expect("stack underflow on JumpIfFalse");
                        match cond {
                            RVal::Bool(true) => {}
                            RVal::Bool(false) => ip = target as usize,
                            other => {
                                let span = chunk.spans[ip - 1];
                                let tname = other.type_name(self.pool);
                                self.pool.release_val(cond);
                                return Err(self.err_runtime_fmt(
                                    "expr.condition",
                                    span,
                                    format_args!("expected bool, got {tname}"),
                                ));
                            }
                        }
                    }
                    Op::JumpIfFalsePeek(target) => {
                        let cond = *self
                            .stack
                            .last()
                            .expect("stack underflow on JumpIfFalsePeek");
                        match cond {
                            RVal::Bool(true) => {
                                self.stack.pop();
                            }
                            RVal::Bool(false) => ip = target as usize,
                            other => {
                                let span = chunk.spans[ip - 1];
                                let tname = other.type_name(self.pool);
                                return Err(self.err_runtime_fmt(
                                    "op.logic",
                                    span,
                                    format_args!("expected bool, got {tname}"),
                                ));
                            }
                        }
                    }
                    Op::JumpIfTruePeek(target) => {
                        let cond = *self
                            .stack
                            .last()
                            .expect("stack underflow on JumpIfTruePeek");
                        match cond {
                            RVal::Bool(false) => {
                                self.stack.pop();
                            }
                            RVal::Bool(true) => ip = target as usize,
                            other => {
                                let span = chunk.spans[ip - 1];
                                let tname = other.type_name(self.pool);
                                return Err(self.err_runtime_fmt(
                                    "op.logic",
                                    span,
                                    format_args!("expected bool, got {tname}"),
                                ));
                            }
                        }
                    }
                    Op::Pop => {
                        let v = self.stack.pop().expect("stack underflow on Op::Pop");
                        self.pool.release_val(v);
                    }
                    Op::TryMatch { pat_idx, fail_ip } => {
                        let val = *self.stack.last().expect("stack underflow on TryMatch");
                        match self.match_pattern(&chunk.patterns[pat_idx as usize], val) {
                            Some(Some(bound)) => {
                                self.pool.retain_val(bound);
                                self.stack_push(bound, chunk.spans[ip - 1])?;
                            }
                            Some(None) => {}
                            None => ip = fail_ip as usize,
                        }
                    }
                    Op::MatchFail => {
                        let span = chunk.spans[ip - 1];
                        return Err(self.err_runtime("expr.match", span, "non-exhaustive match"));
                    }
                    Op::Import(id) => {
                        let span = chunk.spans[ip - 1];
                        if self.frozen {
                            return Err(self.err_runtime(
                                "env.frozen",
                                span,
                                "'import' is not allowed in a frozen environment",
                            ));
                        }
                        let name = self.syms.name(id).unwrap_or("?");
                        let Some(mod_val) = self.modules.get_val(name) else {
                            let out = self.pool.core.error.begin_msg(
                                "expr.module",
                                ErrorKind::Runtime,
                                span,
                            );
                            let _ = write!(out, "unknown module '{name}'");
                            write_suggest(out, name, self.modules.loaded_names());
                            return Err(ErrRecorded);
                        };
                        self.pool.retain_val(mod_val);
                        self.define_top(id, mod_val, span)?;
                        self.stack_push(RVal::Void, span)?;
                    }
                    Op::Export(idx) => {
                        let span = chunk.spans[ip - 1];
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
                        self.stack_push(RVal::Void, span)?;
                    }
                }
            }
            debug_assert_eq!(self.stack.len(), frame_stack_start + 1);
            let value = self.stack.pop().unwrap_or(RVal::Void);
            // 高階組み込みコールバックの高速再利用経路：
            // 親が `Builtin` で、末尾呼び出しによる差し替えがない場合は、
            // 同じクロージャの次要素呼び出しをフレーム生成・深度増減なしでそのまま継続する。
            if frame.tail_source.is_none()
                && matches!(frames.last(), Some(FrameEntry::Builtin(_)))
                && let FrameSource::Proto(proto) = frame.entry
            {
                let Some(FrameEntry::Builtin(bframe)) = frames.last_mut() else {
                    unreachable!();
                };
                let step = match self.resume_callback_builtin(bframe.as_mut(), value) {
                    Ok(s) => s,
                    Err(e) => {
                        self.active_module = frame.prev_mod();
                        if frame.counted_depth {
                            self.leave_depth();
                        }
                        self.release_stack_from(frame.cleanup_stack_to as usize);
                        return Ok(FrameAction::FailUnwound(e));
                    }
                };
                match step {
                    CallbackStep::Invoke {
                        callee,
                        args,
                        argc,
                        ..
                    } if matches!(
                        (callee, bframe.callback),
                        (RVal::Obj(a), RVal::Obj(b)) if a == b
                    ) => {
                        // 同じクロージャへの次要素呼び出し：
                        // スタック長を変えず、引数・ローカルスロットをその場で入れ替えて `ip = 0` へ戻る。
                        let local_count = proto.local_count as usize;
                        for (i, &new_v) in args.iter().enumerate().take(argc) {
                            self.pool.retain_val(new_v);
                            let old_v =
                                std::mem::replace(&mut self.stack.as_mut_slice()[base + i], new_v);
                            self.pool.release_val(old_v);
                        }
                        for i in argc..local_count {
                            let old_v =
                                std::mem::replace(&mut self.stack.as_mut_slice()[base + i], RVal::Void);
                            self.pool.release_val(old_v);
                        }
                        ip = 0;
                        continue 'frame;
                    }
                    CallbackStep::Done(done_val) => {
                        self.active_module = frame.prev_mod();
                        if frame.counted_depth {
                            self.leave_depth();
                        }
                        self.release_stack_from(frame.cleanup_stack_to as usize);
                        let Some(FrameEntry::Builtin(mut done_frame)) = frames.pop() else {
                            unreachable!();
                        };
                        let cleanup_to = done_frame.cleanup_stack_to;
                        self.cleanup_builtin_frame(done_frame.as_mut());
                        self.release_stack_from(cleanup_to);
                        match frames.last() {
                            None => return Ok(FrameAction::Done(done_val)),
                            Some(FrameEntry::Rin(_)) => {
                                let Some(FrameEntry::Rin(parent)) = frames.pop() else {
                                    unreachable!();
                                };
                                *frame = parent;
                                self.stack_push(done_val, Span::default())?;
                                base = frame.base as usize;
                                frame_stack_start = frame.frame_stack_start as usize;
                                ip = frame.ip as usize;
                                continue 'frame;
                            }
                            Some(FrameEntry::Builtin(_)) => {
                                return Ok(FrameAction::ReturnToBuiltin(done_val));
                            }
                        }
                    }
                    CallbackStep::Invoke { .. } => {
                        // コールバック先が異なる場合（通常は起きない）は通常の `drive_frames` へ返す。
                        unreachable!("callback target changed within same BuiltinFrame");
                    }
                }
            }

            self.active_module = frame.prev_mod();
            if frame.counted_depth {
                self.leave_depth();
            }
            self.release_stack_from(frame.cleanup_stack_to as usize);
            match frames.last() {
                None => return Ok(FrameAction::Done(value)),
                Some(FrameEntry::Rin(_)) => {
                    let Some(FrameEntry::Rin(parent)) = frames.pop() else {
                        unreachable!();
                    };
                    *frame = parent;
                    self.stack_push(value, Span::default())?;
                    base = frame.base as usize;
                    frame_stack_start = frame.frame_stack_start as usize;
                    ip = frame.ip as usize;
                    continue 'frame;
                }
                Some(FrameEntry::Builtin(_)) => {
                    return Ok(FrameAction::ReturnToBuiltin(value));
                }
            }
        }
    }
}
