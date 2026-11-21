use std::collections::BTreeMap;
use std::rc::Rc;

use crate::ast::Pattern;
use crate::bytecode::{BuiltinId, Op};
use crate::error::{Error, Span, StackFrame};
use crate::pool::{Obj, RVal};

use super::{CallOrigin, FrameSource, Runner};

impl Runner<'_, '_> {
    pub(super) fn exec_frame(
        &mut self,
        entry: FrameSource<'_>,
        captures: Rc<[RVal]>,
        base: usize,
        local_count: usize,
        origin: Option<CallOrigin<'_>>,
        caller_module: Option<u32>,
    ) -> Result<RVal, Error> {
        let mut tail_source: Option<FrameSource<'_>> = None;
        let mut tail_calls = 0usize;
        let mut res = self.exec_frame_inner(
            &entry,
            &mut tail_source,
            &mut tail_calls,
            captures,
            base,
            local_count,
        );
        if let Err(ref mut err) = res {
            self.record_unwind_frame(
                err,
                &entry,
                tail_source.as_ref(),
                tail_calls,
                origin,
                caller_module,
            );
        }
        res
    }

    #[cold]
    #[inline(never)]
    fn record_unwind_frame(
        &self,
        err: &mut Error,
        entry: &FrameSource<'_>,
        tail_source: Option<&FrameSource<'_>>,
        tail_calls: usize,
        origin: Option<CallOrigin<'_>>,
        caller_module_id: Option<u32>,
    ) {
        let active_mod = match tail_source.unwrap_or(entry) {
            FrameSource::Proto(tp) => tp.module,
            FrameSource::Top(_) => self.active_module,
        };
        err.resolve_module(active_mod.map(|id| self.module_names[id as usize].clone()));
        let (Some(orig), FrameSource::Proto(proto)) = (origin, entry) else {
            return;
        };
        let module = proto
            .module
            .map(|id| self.module_names[id as usize].clone());
        let caller_module = caller_module_id.map(|id| self.module_names[id as usize].clone());
        let (tail_target_module, tail_target_name) = if tail_calls > 0
            && let Some(FrameSource::Proto(tp)) = tail_source
        {
            (
                tp.module.map(|id| self.module_names[id as usize].clone()),
                tp.name.clone(),
            )
        } else {
            (None, None)
        };
        let (at, from_host, via_builtin) = match orig {
            CallOrigin::Rin(s) => (s, false, None),
            CallOrigin::Host => (Span::default(), true, None),
            CallOrigin::Builtin { name, at } => (at, false, Some(name.to_string())),
        };
        err.push_frame(StackFrame {
            module,
            name: proto.name.clone(),
            caller_module,
            at,
            tail_calls,
            tail_target_module,
            tail_target_name,
            repeat_count: 1,
            from_host,
            via_builtin,
        });
    }

    #[inline(always)]
    fn exec_frame_inner(
        &mut self,
        entry: &FrameSource<'_>,
        tail_source: &mut Option<FrameSource<'_>>,
        tail_calls: &mut usize,
        mut captures: Rc<[RVal]>,
        base: usize,
        local_count: usize,
    ) -> Result<RVal, Error> {
        let mut ip = 0usize;
        let mut frame_stack_start = base + local_count;
        'frame: loop {
            let chunk = tail_source.as_ref().unwrap_or(entry).chunk();
            while ip < chunk.code.len() {
                let op = chunk.code[ip];
                ip += 1;

                match op {
                    Op::Tick(start, count) => {
                        self.tick(chunk, start, count)?;
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
                        let v = self.lookup_global(id, span)?;
                        self.stack.push(v);
                    }
                    Op::DefGlobal(id) => {
                        let v = self.stack.pop().expect("stack underflow on DefGlobal");
                        self.define_top(id, v);
                        self.stack.push(RVal::Void);
                    }
                    Op::DefFn(name_id, proto_idx) => {
                        let span = chunk.spans[ip - 1];
                        let proto = &chunk.protos[proto_idx as usize];
                        let closure = self.build_closure(proto, &captures, base, span)?;
                        self.define_top(name_id, closure);
                        self.stack.push(RVal::Void);
                    }
                    Op::MakeClosure(proto_idx) => {
                        let span = chunk.spans[ip - 1];
                        let proto = &chunk.protos[proto_idx as usize];
                        let closure = self.build_closure(proto, &captures, base, span)?;
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
                                span,
                                format!("{name} has no field '{fname}'"),
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
                                            span,
                                            format!("{name} has no field '{field}'"),
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
                                            span,
                                            format!("module '{name}' has no export '{field}'"),
                                        )),
                                    }
                                }
                                _ => Err(Error::runtime(
                                    span,
                                    format!(
                                        "cannot read field '{field}' on {}",
                                        val.type_name(self.pool)
                                    ),
                                )),
                            },
                            other => Err(Error::runtime(
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
                                span,
                                format!("cannot index into {}", list_val.type_name(self.pool)),
                            ))
                        } else if idx < 0 {
                            Err(Error::runtime(
                                span,
                                format!(
                                    "index {idx} out of range (length {})",
                                    self.pool.list_len(list_val)
                                ),
                            ))
                        } else {
                            match self.list_get(list_val, idx as usize) {
                                Some(v) => {
                                    self.pool.retain_val(v);
                                    Ok(v)
                                }
                                None => Err(Error::runtime(
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
                        let callee = self.stack[arg_start - 1];
                        match callee {
                            RVal::Builtin(b) => {
                                let res = self.call_builtin_from_stack(b, arg_start, argc, span);
                                self.stack.push(res?);
                            }
                            RVal::Obj(id) => {
                                let (proto, cap) = match self.pool.get(id) {
                                    Obj::Closure { proto, captures } => {
                                        (Rc::clone(proto), Rc::clone(captures))
                                    }
                                    _ => {
                                        return Err(Error::runtime(
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
                                        span,
                                        format!("expected {} arguments, got {}", proto.arity, argc),
                                    ));
                                }
                                self.enter_depth(span)?;
                                // Leave callee at arg_start - 1 so args stay in place (no memmove).
                                let child_base = arg_start;
                                let child_locals = proto.local_count as usize;
                                self.stack.resize(child_base + child_locals, RVal::Void);
                                let prev_mod =
                                    std::mem::replace(&mut self.active_module, proto.module);
                                let res = self.exec_frame(
                                    FrameSource::Proto(proto),
                                    cap,
                                    child_base,
                                    child_locals,
                                    Some(CallOrigin::Rin(span)),
                                    prev_mod,
                                );
                                self.active_module = prev_mod;
                                self.leave_depth();
                                self.release_stack_from(arg_start - 1);
                                self.stack.push(res?);
                            }
                            other => {
                                return Err(Error::runtime(
                                    span,
                                    format!(
                                        "cannot call value of type {}",
                                        other.type_name(self.pool)
                                    ),
                                ));
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
                                        span,
                                        format!("expected {} arguments, got {}", proto.arity, argc),
                                    ));
                                }
                                // Release old frame's locals and any temporaries below `arg_start - 1`
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
                                self.pool.release_val(callee);
                                self.active_module = proto.module;
                                *tail_source = Some(FrameSource::Proto(proto));
                                *tail_calls += 1;
                                captures = cap;
                                frame_stack_start = base + new_locals;
                                ip = 0;
                                continue 'frame;
                            }
                            RVal::Builtin(b) => {
                                let res = self.call_builtin_from_stack(b, arg_start, argc, span);
                                self.stack.push(res?);
                            }
                            _ => {
                                // Fallback for non-closure or top-level
                                return Err(Error::runtime(
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
                        return Err(Error::runtime(span, "'and' requires at least 1 argument"));
                    }
                    Op::Or => {
                        let span = chunk.spans[ip - 1];
                        return Err(Error::runtime(span, "'or' requires at least 1 argument"));
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
                        return Err(Error::runtime(span, "non-exhaustive match"));
                    }
                    Op::Import(id) => {
                        let span = chunk.spans[ip - 1];
                        let name = &self.syms.names[id as usize];
                        let Some(&mod_val) = self.modules.get(name.as_ref()) else {
                            return Err(Error::runtime(span, format!("unknown module '{name}'")));
                        };
                        self.pool.retain_val(mod_val);
                        self.define_top(id, mod_val);
                        self.stack.push(RVal::Void);
                    }
                    Op::Export(idx) => {
                        let span = chunk.spans[ip - 1];
                        if self.active_module.is_none() {
                            return Err(Error::runtime(
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
        Ok(self.stack.pop().unwrap_or(RVal::Void))
    }

    #[inline(always)]
    pub(super) fn call_builtin_from_stack(
        &mut self,
        b: BuiltinId,
        arg_start: usize,
        argc: usize,
        span: Span,
    ) -> Result<RVal, Error> {
        if argc == 2 {
            let lhs = self.stack[arg_start];
            let rhs = self.stack[arg_start + 1];
            self.stack.truncate(arg_start - 1);
            if let (RVal::Int(x), RVal::Int(y)) = (lhs, rhs) {
                match b {
                    BuiltinId::Add => {
                        return x
                            .checked_add(y)
                            .map(RVal::Int)
                            .ok_or_else(|| Error::runtime(span, "i64 overflow"));
                    }
                    BuiltinId::Sub => {
                        return x
                            .checked_sub(y)
                            .map(RVal::Int)
                            .ok_or_else(|| Error::runtime(span, "i64 overflow"));
                    }
                    BuiltinId::Mul => {
                        return x
                            .checked_mul(y)
                            .map(RVal::Int)
                            .ok_or_else(|| Error::runtime(span, "i64 overflow"));
                    }
                    BuiltinId::Lt => return Ok(RVal::Bool(x < y)),
                    BuiltinId::Le => return Ok(RVal::Bool(x <= y)),
                    BuiltinId::Gt => return Ok(RVal::Bool(x > y)),
                    BuiltinId::Ge => return Ok(RVal::Bool(x >= y)),
                    BuiltinId::Eq => return Ok(RVal::Bool(x == y)),
                    BuiltinId::Ne => return Ok(RVal::Bool(x != y)),
                    _ => {}
                }
            }
            let res = self.apply_builtin(b, &[lhs, rhs], span);
            self.pool.release_val(lhs);
            self.pool.release_val(rhs);
            return res;
        }
        match argc {
            0 => {
                self.stack.truncate(arg_start - 1);
                self.apply_builtin(b, &[], span)
            }
            1 => {
                let a = self.stack[arg_start];
                self.stack.truncate(arg_start - 1);
                let res = self.apply_builtin(b, &[a], span);
                self.pool.release_val(a);
                res
            }
            3 => {
                let a = self.stack[arg_start];
                let c = self.stack[arg_start + 1];
                let d = self.stack[arg_start + 2];
                self.stack.truncate(arg_start - 1);
                let res = self.apply_builtin(b, &[a, c, d], span);
                self.pool.release_val(a);
                self.pool.release_val(c);
                self.pool.release_val(d);
                res
            }
            _ => {
                let args: Vec<RVal> = self.stack.drain(arg_start..).collect();
                self.stack.pop(); // callee
                let res = self.apply_builtin(b, &args, span);
                self.release_all(args);
                res
            }
        }
    }

    pub(super) fn match_pattern(&self, pat: &Pattern, val: RVal) -> Option<Option<RVal>> {
        match (pat, val) {
            (Pattern::Wildcard, _) => Some(None),
            (Pattern::Var(_), v) => Some(Some(v)),
            (Pattern::Int(a), RVal::Int(b)) if *a == b => Some(None),
            (Pattern::Bool(a), RVal::Bool(b)) if *a == b => Some(None),
            (Pattern::Char(a), RVal::Char(b)) if *a == b => Some(None),
            (Pattern::Str(a), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Str(s) if a == s.as_ref() => Some(None),
                _ => None,
            },
            (Pattern::None, RVal::None) => Some(None),
            (Pattern::Some(bind), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Some(inner) => Some(bind.as_ref().map(|_| *inner)),
                _ => None,
            },
            (Pattern::Ok(bind), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Ok(inner) => Some(bind.as_ref().map(|_| *inner)),
                _ => None,
            },
            (Pattern::Err(bind), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Err(inner) => Some(bind.as_ref().map(|_| *inner)),
                _ => None,
            },
            (
                Pattern::Variant {
                    type_name,
                    variant,
                    binding,
                },
                RVal::Obj(id),
            ) => match self.pool.get(id) {
                Obj::Variant {
                    type_name: vt,
                    variant: vv,
                    data,
                } if type_name == vt.as_ref() && variant == vv.as_ref() => match (binding, data) {
                    (Some(_), Some(d)) => Some(Some(*d)),
                    (None, _) => Some(None),
                    (Some(_), None) => None,
                },
                _ => None,
            },
            _ => None,
        }
    }
}
