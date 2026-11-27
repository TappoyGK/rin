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
            let chunk = frame.active_chunk();
            let code = chunk.code.as_slice();
            while ip < code.len() {
                let op = code[ip];
                ip += 1;

                match op {
                    Op::Tick(start, count) => {
                        self.tick(chunk, start, count)?;
                    }
                    Op::Check(equality) => {
                        let span = chunk.spans[ip - 1];
                        self.exec_check(equality, span)?;
                    }
                    Op::Void => self.stack_push_at(RVal::Void, chunk, ip - 1)?,
                    Op::Int(n) => self.stack_push_at(RVal::Int(n), chunk, ip - 1)?,
                    Op::Float(idx) => {
                        self.stack_push_at(RVal::Float(chunk.floats[idx as usize]), chunk, ip - 1)?
                    }
                    Op::Bool(b) => self.stack_push_at(RVal::Bool(b), chunk, ip - 1)?,
                    Op::Char(c) => self.stack_push_at(RVal::Char(c), chunk, ip - 1)?,
                    Op::Str(idx) => {
                        let i = idx as usize;
                        if let Some(id) = chunk.str_obj(i) {
                            self.stack_push_at(RVal::Obj(id), chunk, ip - 1)?;
                        } else {
                            let s = &chunk.strings[i];
                            let span = chunk.spans[ip - 1];
                            let id = self.pool.alloc_str(s.as_ref(), span)?;
                            self.stack_push(RVal::Obj(id), span)?;
                        }
                    }
                    Op::Ident(idx) => {
                        let s = &chunk.strings[idx as usize];
                        let span = chunk.spans[ip - 1];
                        let id = self.pool.alloc_ident(s.as_ref(), span)?;
                        self.stack_push(RVal::Obj(id), span)?;
                    }
                    Op::None => self.stack_push_at(RVal::None, chunk, ip - 1)?,
                    Op::EmptyList => self.stack_push_at(RVal::EmptyList, chunk, ip - 1)?,
                    Op::GetLocal(slot) => {
                        let v = self.stack.get_copy(base + slot as usize);
                        self.pool.retain_val(v);
                        self.stack_push_at(v, chunk, ip - 1)?;
                    }
                    Op::SetLocal(slot) => {
                        let v = self.stack.pop().expect("stack underflow on SetLocal");
                        let old = self.stack.replace_at(base + slot as usize, v);
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
                    Op::MakeListSplice(mask_idx) => {
                        let span = chunk.spans[ip - 1];
                        self.exec_make_list_splice(chunk, mask_idx, span)?;
                    }
                    Op::FormatTemplate(tmpl_idx) => {
                        let span = chunk.spans[ip - 1];
                        self.exec_format_template(chunk, tmpl_idx, span)?;
                    }
                    Op::MakeTuple(n) => {
                        let span = chunk.spans[ip - 1];
                        self.exec_make_tuple(n, span)?;
                    }
                    Op::MakeStruct(s_idx) => {
                        let span = chunk.spans[ip - 1];
                        self.exec_make_struct(chunk, s_idx, span)?;
                    }
                    Op::WithCheckStruct => {
                        let span = chunk.spans[ip - 1];
                        self.exec_with_check_struct(span)?;
                    }
                    Op::WithCheckField(f_idx, pushed_count) => {
                        let span = chunk.spans[ip - 1];
                        self.exec_with_check_field(chunk, f_idx, pushed_count, span)?;
                    }
                    Op::WithFinish(s_idx) => {
                        let span = chunk.spans[ip - 1];
                        self.exec_with_finish(chunk, s_idx, span)?;
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
                        self.exec_get_field(chunk, f_idx, span)?;
                    }
                    Op::GetIndex => {
                        let span = chunk.spans[ip - 1];
                        self.exec_get_index(span)?;
                    }
                    Op::Call(argc) => {
                        let argc = argc as usize;
                        let arg_start = self.stack.len() - argc;
                        let callee = self.stack.get_copy(arg_start - 1);
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
                            frame.ip = ip as u32;
                            frame.frame_stack_start = frame_stack_start as u32;
                            if frames
                                .push_work(
                                    &mut self.pool.work,
                                    &mut self.pool.core,
                                    FrameEntry::Rin(*frame),
                                )
                                .is_none()
                            {
                                self.leave_depth();
                                self.release_stack_from(arg_start - 1);
                                return Err(self.work_oom(span));
                            }
                            let prev_mod = std::mem::replace(&mut self.active_module, proto.module);
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
                        let callee = self.stack.get_copy(arg_start - 1);
                        match callee {
                            RVal::Obj(id)
                                if matches!(frame.active_source(), FrameSource::Proto(_)) =>
                            {
                                let (proto, cap) = match self.pool.get(id) {
                                    Obj::Closure { proto, captures } => (*proto, *captures),
                                    _ => return Err(self.call_type_error(callee, span)),
                                };
                                if proto.arity as usize != argc {
                                    return Err(self.call_arity_error(proto.arity, argc, span));
                                }
                                for i in base..(arg_start - 1) {
                                    let old = self.stack.get_copy(i);
                                    self.pool.release_val(old);
                                }
                                for i in 0..argc {
                                    let arg = self.stack.get_copy(arg_start + i);
                                    self.stack.set_at(base + i, arg);
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
                                frame.set_tail_proto(proto);
                                frame.tail_calls = frame.tail_calls.saturating_add(1);
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
                        self.exec_import(id, span)?;
                    }
                    Op::Export(idx) => {
                        let span = chunk.spans[ip - 1];
                        self.exec_export(chunk, idx, span)?;
                    }
                }
            }
            debug_assert_eq!(self.stack.len(), frame_stack_start + 1);
            let value = self.stack.pop().unwrap_or(RVal::Void);
            // 高階組み込みコールバックの高速再利用経路：
            // 親が `Builtin` で、末尾呼び出しによる差し替えがない場合は、
            // 同じクロージャの次要素呼び出しをフレーム生成・深度増減なしでそのまま継続する。
            if !frame.has_tail()
                && matches!(frames.last(), Some(FrameEntry::Builtin(_)))
                && let FrameSource::Proto(proto) = frame.entry_source()
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
                        callee, args, argc, ..
                    } if matches!(
                        (callee, bframe.callback),
                        (RVal::Obj(a), RVal::Obj(b)) if a == b
                    ) =>
                    {
                        // 同じクロージャへの次要素呼び出し：
                        // スタック長を変えず、引数・ローカルスロットをその場で入れ替えて `ip = 0` へ戻る。
                        let local_count = proto.local_count as usize;
                        for (i, &new_v) in args.iter().enumerate().take(argc) {
                            self.pool.retain_val(new_v);
                            let old_v = self.stack.replace_at(base + i, new_v);
                            self.pool.release_val(old_v);
                        }
                        for i in argc..local_count {
                            let old_v = self.stack.replace_at(base + i, RVal::Void);
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
