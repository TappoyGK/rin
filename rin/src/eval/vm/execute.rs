use super::*;

impl Runner<'_, '_> {
    // Debug では大きな命令ループの作業領域を再帰中に保持しない。
    // Release では最適化で作業領域が小さくなるため、直接呼び出してよい。
    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(super) fn exec_frame_inner(
        &mut self,
        entry: &FrameSource<'_>,
        tail_source: &mut Option<FrameSource<'_>>,
        tail_calls: &mut usize,
        #[cfg(not(debug_assertions))] mut captures: Rc<[RVal]>,
        base: usize,
        #[cfg(not(debug_assertions))] local_count: usize,
        #[cfg(debug_assertions)] resume: &mut Resume,
    ) -> Result<FrameExit, Error> {
        #[cfg(not(debug_assertions))]
        let (mut ip, mut frame_stack_start) = (0, base + local_count);
        #[cfg(debug_assertions)]
        let (mut ip, mut frame_stack_start) = (resume.ip, resume.stack_start);
        #[cfg(debug_assertions)]
        let captures = &mut resume.captures;
        'frame: loop {
            let chunk = tail_source.as_ref().unwrap_or(entry).chunk();
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
                        self.stack.push(RVal::Void);
                    }
                    Op::Void => self.stack.push(RVal::Void),
                    Op::Int(n) => self.stack.push(RVal::Int(n)),
                    Op::Float(idx) => self.stack.push(RVal::Float(chunk.floats[idx as usize])),
                    Op::Bool(b) => self.stack.push(RVal::Bool(b)),
                    Op::Char(c) => self.stack.push(RVal::Char(c)),
                    Op::Str(idx) => {
                        let s = Rc::clone(&chunk.strings[idx as usize]);
                        let span = chunk.spans[ip - 1];
                        let id = self.pool.alloc_obj(Obj::Str(s), span)?;
                        self.stack.push(RVal::Obj(id));
                    }
                    Op::None => self.stack.push(RVal::None),
                    Op::EmptyList => self.stack.push(RVal::EmptyList),
                    Op::GetLocal(slot) => {
                        let v = self.stack[base + slot as usize];
                        self.pool.retain_val(v);
                        self.stack.push(v);
                    }
                    Op::SetLocal(slot) => {
                        let v = self.stack.pop().expect("stack underflow on SetLocal");
                        let old = std::mem::replace(&mut self.stack[base + slot as usize], v);
                        self.pool.release_val(old);
                    }
                    Op::GetCapture(idx) => {
                        let v = captures[idx as usize];
                        self.pool.retain_val(v);
                        self.stack.push(v);
                    }
                    Op::GetGlobal(id) => {
                        let span = chunk.spans[ip - 1];
                        let v = self.lookup_global(id, span, chunk, ip - 1)?;
                        self.stack.push(v);
                    }
                    Op::DefGlobal(id) => {
                        let span = chunk.spans[ip - 1];
                        let v = self.stack.pop().expect("stack underflow on DefGlobal");
                        if self.frozen {
                            self.pool.release_val(v);
                            return Err(Error::runtime(
                                "env.frozen",
                                span,
                                "'def' is not allowed in a frozen environment",
                            ));
                        }
                        self.define_top(id, v);
                        self.stack.push(RVal::Void);
                    }
                    Op::DefFn(name_id, proto_idx) => {
                        let span = chunk.spans[ip - 1];
                        if self.frozen {
                            return Err(Error::runtime(
                                "env.frozen",
                                span,
                                "'defn' is not allowed in a frozen environment",
                            ));
                        }
                        let proto = &chunk.protos[proto_idx as usize];
                        let closure = self.build_closure(proto, captures.as_ref(), base, span)?;
                        self.define_top(name_id, closure);
                        self.stack.push(RVal::Void);
                    }
                    Op::MakeClosure(proto_idx) => {
                        let span = chunk.spans[ip - 1];
                        let proto = &chunk.protos[proto_idx as usize];
                        let closure = self.build_closure(proto, captures.as_ref(), base, span)?;
                        self.stack.push(closure);
                    }
                    Op::MakeList(n) => {
                        let span = chunk.spans[ip - 1];
                        let start = self.stack.len() - n as usize;
                        let list = self.build_list_from_owned_slice(start, span)?;
                        self.stack.push(list);
                    }
                    Op::MakeTuple(n) => {
                        let span = chunk.spans[ip - 1];
                        let start = self.stack.len() - n as usize;
                        let items: Vec<RVal> = self.stack.drain(start..).collect();
                        let id = self.pool.alloc_obj(Obj::Tuple(items), span)?;
                        self.stack.push(RVal::Obj(id));
                    }
                    Op::MakeStruct(s_idx) => {
                        let span = chunk.spans[ip - 1];
                        let desc = &chunk.structs[s_idx as usize];
                        let start = self.stack.len() - desc.fields.len();
                        let mut map = BTreeMap::new();
                        for (idx, fname) in desc.fields.iter().enumerate() {
                            let val = self.stack[start + idx];
                            if let Some(prev) = map.insert(Rc::clone(fname), val) {
                                self.pool.release_val(prev);
                            }
                        }
                        self.stack.truncate(start);
                        let id = self.pool.alloc_obj(
                            Obj::Struct {
                                name: Rc::clone(&desc.name),
                                fields: map,
                            },
                            span,
                        )?;
                        self.stack.push(RVal::Obj(id));
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
                            return Err(Error::runtime(
                                "expr.with",
                                span,
                                format!("'with' requires a struct, got {tname}"),
                            ));
                        }
                    }
                    Op::WithCheckField(f_idx, pushed_count) => {
                        let span = chunk.spans[ip - 1];
                        let fname = &chunk.strings[f_idx as usize];
                        let base_val = self.stack[self.stack.len() - 1 - pushed_count as usize];
                        let RVal::Obj(id) = base_val else {
                            unreachable!();
                        };
                        let Obj::Struct { name, fields } = self.pool.get(id) else {
                            unreachable!();
                        };
                        if !fields.contains_key(fname.as_ref()) {
                            return Err(Error::runtime(
                                "expr.with",
                                span,
                                available(
                                    format!("{name} has no field '{fname}'"),
                                    "fields",
                                    fields.keys().map(|name| name.as_ref()),
                                ),
                            ));
                        }
                    }
                    Op::WithFinish(s_idx) => {
                        let span = chunk.spans[ip - 1];
                        let desc = &chunk.structs[s_idx as usize];
                        let n = desc.fields.len();
                        let val_start = self.stack.len() - n;
                        let base_val = self.stack[val_start - 1];
                        let RVal::Obj(id) = base_val else {
                            unreachable!();
                        };
                        let Obj::Struct { name, fields } = self.pool.get(id) else {
                            unreachable!();
                        };
                        if !super::super::runner::try_charge_steps(
                            &mut self.steps_left,
                            fields.len(),
                        ) {
                            return self.step_limit_result(span);
                        }
                        let name = name.clone();
                        let mut updated = fields.clone();
                        for &v in updated.values() {
                            self.pool.retain_val(v);
                        }
                        for (idx, fname) in desc.fields.iter().enumerate() {
                            let val = self.stack[val_start + idx];
                            if let Some(prev) = updated.insert(Rc::clone(fname), val) {
                                self.pool.release_val(prev);
                            }
                        }
                        self.stack.truncate(val_start - 1);
                        self.pool.release_val(base_val);
                        let new_id = self.pool.alloc_obj(
                            Obj::Struct {
                                name,
                                fields: updated,
                            },
                            span,
                        )?;
                        self.stack.push(RVal::Obj(new_id));
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
                        let id = self.pool.alloc_obj(
                            Obj::Variant {
                                type_name: Rc::clone(&chunk.strings[type_name as usize]),
                                variant: Rc::clone(&chunk.strings[variant as usize]),
                                data,
                            },
                            span,
                        )?;
                        self.stack.push(RVal::Obj(id));
                    }
                    Op::MakeSome => {
                        let span = chunk.spans[ip - 1];
                        let v = self.stack.pop().expect("stack underflow on MakeSome");
                        let id = self.pool.alloc_obj(Obj::Some(v), span)?;
                        self.stack.push(RVal::Obj(id));
                    }
                    Op::MakeOk => {
                        let span = chunk.spans[ip - 1];
                        let v = self.stack.pop().expect("stack underflow on MakeOk");
                        let id = self.pool.alloc_obj(Obj::Ok(v), span)?;
                        self.stack.push(RVal::Obj(id));
                    }
                    Op::MakeErr => {
                        let span = chunk.spans[ip - 1];
                        let v = self.stack.pop().expect("stack underflow on MakeErr");
                        let id = self.pool.alloc_obj(Obj::Err(v), span)?;
                        self.stack.push(RVal::Obj(id));
                    }
                    Op::GetField(f_idx) => {
                        let span = chunk.spans[ip - 1];
                        let field = &chunk.strings[f_idx as usize];
                        let val = self.stack.pop().expect("stack underflow on GetField");
                        let res = match val {
                            RVal::Obj(id) => match self.pool.get(id) {
                                Obj::Struct { name, fields } => {
                                    match fields.get(field.as_ref()).copied() {
                                        Some(fv) => {
                                            self.pool.retain_val(fv);
                                            Ok(fv)
                                        }
                                        None => Err(Error::runtime(
                                            "expr.field",
                                            span,
                                            available(
                                                format!("{name} has no field '{field}'"),
                                                "fields",
                                                fields.keys().map(|name| name.as_ref()),
                                            ),
                                        )),
                                    }
                                }
                                Obj::Module { name, exports } => {
                                    match exports.get(field.as_ref()).copied() {
                                        Some(fv) => {
                                            self.pool.retain_val(fv);
                                            Ok(fv)
                                        }
                                        None => Err(Error::runtime(
                                            "expr.module",
                                            span,
                                            available(
                                                format!("module '{name}' has no export '{field}'"),
                                                "exports",
                                                exports.keys().map(|name| name.as_ref()),
                                            ),
                                        )),
                                    }
                                }
                                _ => Err(Error::runtime(
                                    "expr.field",
                                    span,
                                    format!(
                                        "cannot read field '{field}' on {}",
                                        val.type_name(self.pool)
                                    ),
                                )),
                            },
                            other => Err(Error::runtime(
                                "expr.field",
                                span,
                                format!(
                                    "cannot read field '{field}' on {}",
                                    other.type_name(self.pool)
                                ),
                            )),
                        };
                        self.pool.release_val(val);
                        self.stack.push(res?);
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
                                return Err(Error::runtime(
                                    "expr.index",
                                    span,
                                    format!("index must be i64, got {tname}"),
                                ));
                            }
                        };
                        self.pool.release_val(idx_val);

                        let res = if let RVal::Obj(id) = list_val
                            && let Obj::Array(arr) = self.pool.get(id)
                        {
                            if idx < 0 || (idx as usize) >= arr.len() {
                                Err(Error::runtime(
                                    "expr.index",
                                    span,
                                    format!("index {idx} out of range (length {})", arr.len()),
                                ))
                            } else {
                                let v = arr[idx as usize];
                                self.pool.retain_val(v);
                                Ok(v)
                            }
                        } else if !self.is_list(list_val) {
                            Err(Error::runtime(
                                "expr.index",
                                span,
                                format!("cannot index into {}", list_val.type_name(self.pool)),
                            ))
                        } else if idx < 0 {
                            Err(Error::runtime(
                                "expr.index",
                                span,
                                format!(
                                    "index {idx} out of range (length {})",
                                    self.pool.list_len(list_val)
                                ),
                            ))
                        } else {
                            match self.list_get(list_val, idx as usize, span) {
                                Err(e) => Err(e),
                                Ok(Some(v)) => {
                                    self.pool.retain_val(v);
                                    Ok(v)
                                }
                                Ok(None) => Err(Error::runtime(
                                    "expr.index",
                                    span,
                                    format!(
                                        "index {idx} out of range (length {})",
                                        self.pool.list_len(list_val)
                                    ),
                                )),
                            }
                        };
                        self.pool.release_val(list_val);
                        self.stack.push(res?);
                    }
                    Op::Call(argc) => {
                        let span = chunk.spans[ip - 1];
                        let argc = argc as usize;
                        let arg_start = self.stack.len() - argc;
                        #[cfg(debug_assertions)]
                        {
                            if let RVal::Builtin(b) = self.stack[arg_start - 1] {
                                #[cfg(debug_assertions)]
                                if b.has_callback() {
                                    resume.ip = ip;
                                    resume.stack_start = frame_stack_start;
                                    return Ok(FrameExit::Call {
                                        arg_start,
                                        argc,
                                        span,
                                    });
                                }
                                let res = self.call_builtin_from_stack(b, arg_start, argc, span);
                                self.stack.push(res?);
                            } else {
                                #[cfg(debug_assertions)]
                                {
                                    resume.ip = ip;
                                    resume.stack_start = frame_stack_start;
                                    return Ok(FrameExit::Call {
                                        arg_start,
                                        argc,
                                        span,
                                    });
                                }
                            }
                        }
                        #[cfg(not(debug_assertions))]
                        {
                            let callee = self.stack[arg_start - 1];
                            match callee {
                                RVal::Builtin(b) => {
                                    let res =
                                        self.call_builtin_from_stack(b, arg_start, argc, span);
                                    self.stack.push(res?);
                                }
                                RVal::Obj(id) => {
                                    call_closure!(self, id, callee, arg_start, argc, span, res => self.stack.push(res?));
                                }
                                other => {
                                    return Err(Error::runtime(
                                        "expr.call",
                                        span,
                                        format!(
                                            "cannot call value of type {}",
                                            other.type_name(self.pool)
                                        ),
                                    ));
                                }
                            }
                        }
                    }

                    Op::TailCall(argc) => {
                        let span = chunk.spans[ip - 1];
                        let argc = argc as usize;
                        let arg_start = self.stack.len() - argc;
                        let callee = self.stack[arg_start - 1];
                        match callee {
                            RVal::Obj(id)
                                if matches!(
                                    tail_source.as_ref().unwrap_or(entry),
                                    FrameSource::Proto(_)
                                ) =>
                            {
                                let (proto, cap) = match self.pool.get(id) {
                                    Obj::Closure { proto, captures } => {
                                        (Rc::clone(proto), Rc::clone(captures))
                                    }
                                    _ => {
                                        return Err(Error::runtime(
                                            "expr.call",
                                            span,
                                            format!(
                                                "cannot call value of type {}",
                                                callee.type_name(self.pool)
                                            ),
                                        ));
                                    }
                                };
                                if proto.arity as usize != argc {
                                    return Err(Error::runtime(
                                        "expr.call",
                                        span,
                                        format!("expected {} arguments, got {}", proto.arity, argc),
                                    ));
                                }
                                // 旧フレームのローカル変数と `arg_start - 1` 未満の一時値を解放する。
                                for i in base..(arg_start - 1) {
                                    let old = self.stack[i];
                                    self.pool.release_val(old);
                                }
                                for i in 0..argc {
                                    self.stack[base + i] = self.stack[arg_start + i];
                                }
                                self.stack.truncate(base + argc);
                                let new_locals = proto.local_count as usize;
                                self.stack.resize(base + new_locals, RVal::Void);
                                // 捕捉値の所有者をローカル領域の直後に残す。
                                // 通常の命令はこの位置より上を使い、次のフレーム置換や
                                // 呼び出し終了時のスタック巻き戻しで一緒に解放する。
                                // 捕捉値がなければ、従来どおりここで解放できる。
                                if cap.is_empty() {
                                    self.pool.release_val(callee);
                                } else {
                                    self.stack.push(callee);
                                }
                                self.active_module = proto.module;
                                *tail_source = Some(FrameSource::Proto(proto));
                                *tail_calls += 1;
                                #[cfg(not(debug_assertions))]
                                {
                                    captures = cap;
                                }
                                #[cfg(debug_assertions)]
                                {
                                    *captures = cap;
                                }
                                frame_stack_start = self.stack.len();
                                ip = 0;
                                continue 'frame;
                            }
                            RVal::Builtin(b) => {
                                #[cfg(debug_assertions)]
                                if b.has_callback() {
                                    resume.ip = ip;
                                    resume.stack_start = frame_stack_start;
                                    return Ok(FrameExit::Call {
                                        arg_start,
                                        argc,
                                        span,
                                    });
                                }
                                let res = self.call_builtin_from_stack(b, arg_start, argc, span);
                                self.stack.push(res?);
                            }
                            _ => {
                                // クロージャ以外の呼び出し対象に対するエラー経路。
                                return Err(Error::runtime(
                                    "expr.call",
                                    span,
                                    format!(
                                        "cannot call value of type {}",
                                        callee.type_name(self.pool)
                                    ),
                                ));
                            }
                        }
                    }
                    Op::And => {
                        let span = chunk.spans[ip - 1];
                        return Err(Error::runtime(
                            "op.logic",
                            span,
                            "'and' requires at least 1 argument",
                        ));
                    }
                    Op::Or => {
                        let span = chunk.spans[ip - 1];
                        return Err(Error::runtime(
                            "op.logic",
                            span,
                            "'or' requires at least 1 argument",
                        ));
                    }
                    Op::Jump(target) => {
                        ip = target;
                    }
                    Op::JumpIfFalse(target) => {
                        let cond = self.stack.pop().expect("stack underflow on JumpIfFalse");
                        match cond {
                            RVal::Bool(true) => {}
                            RVal::Bool(false) => ip = target,
                            other => {
                                let span = chunk.spans[ip - 1];
                                let tname = other.type_name(self.pool);
                                self.pool.release_val(cond);
                                return Err(Error::runtime(
                                    "expr.condition",
                                    span,
                                    format!("expected bool, got {tname}"),
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
                            RVal::Bool(false) => ip = target,
                            other => {
                                let span = chunk.spans[ip - 1];
                                let tname = other.type_name(self.pool);
                                return Err(Error::runtime(
                                    "op.logic",
                                    span,
                                    format!("expected bool, got {tname}"),
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
                            RVal::Bool(true) => ip = target,
                            other => {
                                let span = chunk.spans[ip - 1];
                                let tname = other.type_name(self.pool);
                                return Err(Error::runtime(
                                    "op.logic",
                                    span,
                                    format!("expected bool, got {tname}"),
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
                                self.stack.push(bound);
                            }
                            Some(None) => {}
                            None => ip = fail_ip,
                        }
                    }
                    Op::MatchFail => {
                        let span = chunk.spans[ip - 1];
                        return Err(Error::runtime("expr.match", span, "non-exhaustive match"));
                    }
                    Op::Import(id) => {
                        let span = chunk.spans[ip - 1];
                        if self.frozen {
                            return Err(Error::runtime(
                                "env.frozen",
                                span,
                                "'import' is not allowed in a frozen environment",
                            ));
                        }
                        let name = &self.syms.names[id as usize];
                        let Some(&mod_val) = self.modules.get(name.as_ref()) else {
                            return Err(Error::runtime(
                                "expr.module",
                                span,
                                suggest(
                                    format!("unknown module '{name}'"),
                                    name,
                                    self.modules.keys().map(String::as_str),
                                ),
                            ));
                        };
                        self.pool.retain_val(mod_val);
                        self.define_top(id, mod_val);
                        self.stack.push(RVal::Void);
                    }
                    Op::Export(idx) => {
                        let span = chunk.spans[ip - 1];
                        if self.frozen {
                            return Err(Error::runtime(
                                "env.frozen",
                                span,
                                "'export' is not allowed in a frozen environment",
                            ));
                        }
                        if self.active_module.is_none() {
                            return Err(Error::runtime(
                                "expr.module",
                                span,
                                "'export' can only be used inside a module",
                            ));
                        }
                        let names = &chunk.export_lists[idx as usize];
                        let list = self.export_list.get_or_insert_with(Vec::new);
                        list.extend(names.iter().cloned());
                        self.stack.push(RVal::Void);
                    }
                }
            }
            break;
        }
        debug_assert_eq!(self.stack.len(), frame_stack_start + 1);
        let value = self.stack.pop().unwrap_or(RVal::Void);
        #[cfg(debug_assertions)]
        return Ok(FrameExit::Return(value));
        #[cfg(not(debug_assertions))]
        Ok(value)
    }
}
