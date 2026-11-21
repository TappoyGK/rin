use std::collections::BTreeMap;
use std::rc::Rc;

use crate::alloc::{Allocator, HeapAllocator};
use crate::ast::Pattern;
use crate::bytecode::{BuiltinId, Capture, Chunk, FnProto, Op};
use crate::compiler::compile_program;
use crate::error::{Error, ErrorKind, Limits, Span};
use crate::parser::parse_program;
use crate::pool::{Obj, Pool, RVal};
use crate::value::Value;

/// Evaluates Rin source code using the default `HeapAllocator`.
pub fn eval_with(
    source: &str,
    bindings: &[(&str, Value)],
    limits: Limits,
) -> Result<Value, Error> {
    let mut alloc = HeapAllocator;
    eval_with_allocator(source, bindings, limits, &mut alloc)
}

/// Evaluates Rin source code using a caller-provided `Allocator`.
pub fn eval_with_allocator(
    source: &str,
    bindings: &[(&str, Value)],
    limits: Limits,
    alloc: &mut dyn Allocator,
) -> Result<Value, Error> {
    let mut machine = Machine::new(limits, alloc);
    for (k, v) in bindings {
        machine.bind(k, v)?;
    }
    machine.eval(source)
}

/// Persistent Rin execution environment backed by a caller-provided `Allocator`.
///
/// Top-level definitions (`def` / `defn`) persist across `eval` calls, while
/// temporary values created during evaluation are reclaimed via reference
/// counting and reused from the pool's free-list.
pub struct Machine<'a> {
    limits: Limits,
    pool: Pool<'a>,
    globals: BTreeMap<String, RVal>,
    modules: BTreeMap<String, RVal>,
    module_envs: BTreeMap<String, BTreeMap<String, RVal>>,
}

impl<'a> Machine<'a> {
    pub fn new(limits: Limits, alloc: &'a mut dyn Allocator) -> Self {
        let mut globals = BTreeMap::new();
        for &b in BuiltinId::ALL {
            globals.insert(b.name().to_string(), RVal::Builtin(b));
        }
        globals.insert("pi".to_string(), RVal::Float(std::f64::consts::PI));
        Self {
            limits,
            pool: Pool::new(alloc, limits.memory_bytes),
            globals,
            modules: BTreeMap::new(),
            module_envs: BTreeMap::new(),
        }
    }

    pub fn bind(&mut self, name: &str, value: &Value) -> Result<(), Error> {
        let rval = self.pool.import(value, Span::default())?;
        if let Some(prev) = self.globals.insert(name.to_string(), rval) {
            self.pool.release_val(prev);
        }
        Ok(())
    }

    pub fn live_bytes(&self) -> usize {
        self.pool.live_bytes()
    }

    /// Loads and evaluates a named module from host-provided source.
    pub fn load_module(&mut self, name: &str, source: &str) -> Result<(), Error> {
        let exprs = parse_program(source)?;
        let chunk = compile_program(&exprs, Some(name))?;
        if let Some(prev_env) = self.module_envs.insert(name.to_string(), BTreeMap::new()) {
            for v in prev_env.into_values() {
                self.pool.release_val(v);
            }
        }
        let mut runner = Runner::new(
            self.limits,
            &mut self.pool,
            &mut self.globals,
            &self.modules,
            &mut self.module_envs,
            Some(Rc::from(name)),
        );
        let v = runner.run_top_chunk(&chunk)?;
        runner.pool.release_val(v);
        let export_filter = runner.export_list.take();
        let mod_env = self.module_envs.get(name).unwrap();
        let mut exports = BTreeMap::new();
        match export_filter {
            Some(names) => {
                for sym in names {
                    let Some(&val) = mod_env.get(&sym) else {
                        return Err(Error::runtime(
                            Span::default(),
                            format!("module '{name}' cannot export unknown symbol '{sym}'"),
                        ));
                    };
                    self.pool.retain_val(val);
                    exports.insert(sym, val);
                }
            }
            None => {
                for (sym, &val) in mod_env {
                    let is_module = matches!(val, RVal::Obj(id) if matches!(self.pool.get(id), Obj::Module { .. }));
                    if !is_module {
                        self.pool.retain_val(val);
                        exports.insert(sym.clone(), val);
                    }
                }
            }
        }
        let mod_id = match self.pool.alloc_obj(
            Obj::Module {
                name: name.to_string(),
                exports: exports.clone(),
            },
            Span::default(),
        ) {
            Ok(id) => id,
            Err(e) => {
                for v in exports.into_values() {
                    self.pool.release_val(v);
                }
                return Err(e);
            }
        };
        if let Some(prev) = self.modules.insert(name.to_string(), RVal::Obj(mod_id)) {
            self.pool.release_val(prev);
        }
        Ok(())
    }

    pub fn eval(&mut self, source: &str) -> Result<Value, Error> {
        let exprs = parse_program(source)?;
        let chunk = compile_program(&exprs, None)?;
        let mut runner = Runner::new(
            self.limits,
            &mut self.pool,
            &mut self.globals,
            &self.modules,
            &mut self.module_envs,
            None,
        );
        let last = runner.run_top_chunk(&chunk)?;
        let exported = runner.pool.export(last);
        runner.pool.release_val(last);
        Ok(exported)
    }

    /// Calls a previously defined global function directly without re-parsing source.
    pub fn call(&mut self, fn_name: &str, args: &[Value]) -> Result<Value, Error> {
        let Some(&callee) = self.globals.get(fn_name) else {
            return Err(Error::runtime(
                Span::default(),
                format!("unknown function '{fn_name}'"),
            ));
        };
        self.call_rval(callee, args)
    }

    /// Calls an exported function from a loaded module directly without re-parsing source.
    pub fn call_in(&mut self, mod_name: &str, fn_name: &str, args: &[Value]) -> Result<Value, Error> {
        let Some(&RVal::Obj(mod_id)) = self.modules.get(mod_name) else {
            return Err(Error::runtime(
                Span::default(),
                format!("unknown module '{mod_name}'"),
            ));
        };
        let Obj::Module { exports, .. } = self.pool.get(mod_id) else {
            unreachable!();
        };
        let Some(&callee) = exports.get(fn_name) else {
            return Err(Error::runtime(
                Span::default(),
                format!("module '{mod_name}' has no export '{fn_name}'"),
            ));
        };
        self.call_rval(callee, args)
    }

    fn call_rval(&mut self, callee: RVal, args: &[Value]) -> Result<Value, Error> {
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
        let mut runner = Runner::new(
            self.limits,
            &mut self.pool,
            &mut self.globals,
            &self.modules,
            &mut self.module_envs,
            None,
        );
        let res = runner.apply(callee, &r_args, Span::default());
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
        for env in old_mod_envs.into_values() {
            for v in env.into_values() {
                self.pool.release_val(v);
            }
        }
        let old = std::mem::take(&mut self.globals);
        for v in old.into_values() {
            self.pool.release_val(v);
        }
    }
}

struct Runner<'a, 'p> {
    limits: Limits,
    steps: usize,
    depth: usize,
    pool: &'a mut Pool<'p>,
    globals: &'a mut BTreeMap<String, RVal>,
    modules: &'a BTreeMap<String, RVal>,
    module_envs: &'a mut BTreeMap<String, BTreeMap<String, RVal>>,
    active_module: Option<Rc<str>>,
    export_list: Option<Vec<String>>,
    stack: Vec<RVal>,
}

impl<'a, 'p> Runner<'a, 'p> {
    fn new(
        limits: Limits,
        pool: &'a mut Pool<'p>,
        globals: &'a mut BTreeMap<String, RVal>,
        modules: &'a BTreeMap<String, RVal>,
        module_envs: &'a mut BTreeMap<String, BTreeMap<String, RVal>>,
        active_module: Option<Rc<str>>,
    ) -> Self {
        Self {
            limits,
            steps: 0,
            depth: 0,
            pool,
            globals,
            modules,
            module_envs,
            active_module,
            export_list: None,
            stack: Vec::with_capacity(64),
        }
    }

    #[inline(always)]
    fn tick(&mut self, chunk: &Chunk, ip: usize) -> Result<(), Error> {
        self.steps += 1;
        if self.steps > self.limits.steps {
            return Err(self.step_limit_err(chunk.spans[ip - 1]));
        }
        Ok(())
    }

    #[cold]
    fn step_limit_err(&self, span: Span) -> Error {
        Error::new(
            ErrorKind::TooManySteps,
            span,
            format!("step limit exceeded ({})", self.limits.steps),
        )
    }

    fn enter_depth(&mut self, span: Span) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > self.limits.depth {
            return Err(self.depth_limit_err(span));
        }
        Ok(())
    }

    #[cold]
    fn depth_limit_err(&self, span: Span) -> Error {
        Error::new(
            ErrorKind::TooDeep,
            span,
            format!("call depth limit exceeded ({})", self.limits.depth),
        )
    }

    fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn lookup_global(&mut self, name: &str, span: Span) -> Result<RVal, Error> {
        if let Some(mod_name) = &self.active_module
            && let Some(mod_env) = self.module_envs.get(mod_name.as_ref())
            && let Some(&v) = mod_env.get(name)
        {
            self.pool.retain_val(v);
            return Ok(v);
        }
        if let Some(&v) = self.globals.get(name) {
            self.pool.retain_val(v);
            return Ok(v);
        }
        Err(Error::runtime(span, format!("unknown name '{name}'")))
    }

    fn define_top(&mut self, name: String, val: RVal) {
        let prev = if let Some(mod_name) = &self.active_module {
            self.module_envs
                .get_mut(mod_name.as_ref())
                .unwrap()
                .insert(name, val)
        } else {
            self.globals.insert(name, val)
        };
        if let Some(p) = prev {
            self.pool.release_val(p);
        }
    }

    fn release_all(&mut self, vals: impl IntoIterator<Item = RVal>) {
        for v in vals {
            self.pool.release_val(v);
        }
    }

    fn release_stack_from(&mut self, from: usize) {
        while self.stack.len() > from {
            let v = self.stack.pop().unwrap();
            self.pool.release_val(v);
        }
    }

    fn build_closure(
        &mut self,
        proto: &Rc<FnProto>,
        captures: &[RVal],
        base: usize,
        span: Span,
    ) -> Result<RVal, Error> {
        let mut cap_vals = Vec::with_capacity(proto.captures.len());
        for cap in &proto.captures {
            let v = match cap {
                Capture::Local(slot) => self.stack[base + *slot as usize],
                Capture::Upvalue(idx) => captures[*idx as usize],
            };
            self.pool.retain_val(v);
            cap_vals.push(v);
        }
        let id = self.pool.alloc_obj(
            Obj::Closure {
                proto: Rc::clone(proto),
                captures: Rc::from(cap_vals),
            },
            span,
        )?;
        Ok(RVal::Obj(id))
    }

    fn run_top_chunk(&mut self, chunk: &Chunk) -> Result<RVal, Error> {
        let base = self.stack.len();
        self.stack
            .resize(base + chunk.local_count as usize, RVal::Void);
        let res = self.exec_frame(
            FrameSource::Top(chunk),
            Rc::from([]),
            base,
            chunk.local_count as usize,
        );
        match res {
            Ok(val) => {
                debug_assert_eq!(self.stack.len(), base + chunk.local_count as usize);
                self.release_stack_from(base);
                Ok(val)
            }
            Err(e) => {
                self.release_stack_from(base);
                Err(e)
            }
        }
    }

    fn apply(&mut self, callee: RVal, args: &[RVal], span: Span) -> Result<RVal, Error> {
        match callee {
            RVal::Obj(id) => {
                let (proto, captures) = match self.pool.get(id) {
                    Obj::Closure { proto, captures } => (Rc::clone(proto), Rc::clone(captures)),
                    _ => {
                        return Err(Error::runtime(
                            span,
                            format!("cannot call value of type {}", callee.type_name(self.pool)),
                        ));
                    }
                };
                if proto.arity as usize != args.len() {
                    return Err(Error::runtime(
                        span,
                        format!("expected {} arguments, got {}", proto.arity, args.len()),
                    ));
                }
                self.enter_depth(span)?;
                let prev_mod = std::mem::replace(&mut self.active_module, proto.module.clone());
                let base = self.stack.len();
                let local_count = proto.local_count as usize;
                self.stack.reserve(local_count);
                for &val in args {
                    self.pool.retain_val(val);
                    self.stack.push(val);
                }
                self.stack.resize(base + local_count, RVal::Void);
                let res = self.exec_frame(
                    FrameSource::Proto(proto),
                    captures,
                    base,
                    local_count,
                );
                self.active_module = prev_mod;
                self.leave_depth();
                match res {
                    Ok(val) => {
                        self.release_stack_from(base);
                        Ok(val)
                    }
                    Err(e) => {
                        self.release_stack_from(base);
                        Err(e)
                    }
                }
            }
            RVal::Builtin(b) => self.apply_builtin(b, args, span),
            other => Err(Error::runtime(
                span,
                format!("cannot call value of type {}", other.type_name(self.pool)),
            )),
        }
    }

    fn exec_frame(
        &mut self,
        mut source: FrameSource<'_>,
        mut captures: Rc<[RVal]>,
        base: usize,
        local_count: usize,
    ) -> Result<RVal, Error> {
        let mut ip = 0usize;
        let frame_stack_start = base + local_count;

        loop {
            let chunk = source.chunk();
            if ip >= chunk.code.len() {
                break;
            }
            let op = chunk.code[ip];
            ip += 1;

            match op {
                Op::Tick => {
                    self.tick(chunk, ip)?;
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
                Op::GetGlobal(idx) => {
                    let span = chunk.spans[ip - 1];
                    let v = self.lookup_global(&chunk.strings[idx as usize], span)?;
                    self.stack.push(v);
                }
                Op::DefGlobal(idx) => {
                    let v = self.stack.pop().expect("stack underflow on DefGlobal");
                    self.define_top(chunk.strings[idx as usize].to_string(), v);
                    self.stack.push(RVal::Void);
                }
                Op::DefFn(name_idx, proto_idx) => {
                    let span = chunk.spans[ip - 1];
                    let proto = &chunk.protos[proto_idx as usize];
                    let closure = self.build_closure(proto, &captures, base, span)?;
                    self.define_top(chunk.strings[name_idx as usize].to_string(), closure);
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
                        if let Some(prev) = map.insert(fname.to_string(), val) {
                            self.pool.release_val(prev);
                        }
                    }
                    self.stack.truncate(start);
                    let id = self.pool.alloc_obj(
                        Obj::Struct {
                            name: desc.name.to_string(),
                            fields: map,
                        },
                        span,
                    )?;
                    self.stack.push(RVal::Obj(id));
                }
                Op::WithCheckStruct => {
                    let span = chunk.spans[ip - 1];
                    let base_val = *self.stack.last().expect("stack underflow on WithCheckStruct");
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
                        if let Some(prev) = updated.insert(fname.to_string(), val) {
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
                            type_name: chunk.strings[type_name as usize].to_string(),
                            variant: chunk.strings[variant as usize].to_string(),
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
                            Obj::Struct { name, fields } => match fields.get(field.as_ref()).copied()
                            {
                                Some(fv) => {
                                    self.pool.retain_val(fv);
                                    Ok(fv)
                                }
                                None => Err(Error::runtime(
                                    span,
                                    format!("{name} has no field '{field}'"),
                                )),
                            },
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
                            let args: Vec<RVal> = self.stack.drain(arg_start..).collect();
                            self.stack.pop(); // callee
                            let res = self.apply_builtin(b, &args, span);
                            self.release_all(args);
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
                                    format!(
                                        "expected {} arguments, got {}",
                                        proto.arity, argc
                                    ),
                                ));
                            }
                            self.enter_depth(span)?;
                            // Remove callee slot and shift args down by 1 in place
                            self.stack.remove(arg_start - 1);
                            let child_base = arg_start - 1;
                            let child_locals = proto.local_count as usize;
                            self.stack.resize(child_base + child_locals, RVal::Void);
                            let prev_mod =
                                std::mem::replace(&mut self.active_module, proto.module.clone());
                            let res = self.exec_frame(
                                FrameSource::Proto(proto),
                                cap,
                                child_base,
                                child_locals,
                            );
                            self.active_module = prev_mod;
                            self.leave_depth();
                            self.release_stack_from(child_base);
                            self.pool.release_val(callee);
                            self.stack.push(res?);
                        }
                        other => {
                            return Err(Error::runtime(
                                span,
                                format!("cannot call value of type {}", other.type_name(self.pool)),
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
                        RVal::Obj(id) if matches!(source, FrameSource::Proto(_)) => {
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
                                    format!(
                                        "expected {} arguments, got {}",
                                        proto.arity, argc
                                    ),
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
                            self.active_module = proto.module.clone();
                            source = FrameSource::Proto(proto);
                            captures = cap;
                            ip = 0;
                        }
                        RVal::Builtin(b) => {
                            let args: Vec<RVal> = self.stack.drain(arg_start..).collect();
                            self.stack.pop(); // callee
                            let res = self.apply_builtin(b, &args, span);
                            self.release_all(args);
                            self.stack.push(res?);
                        }
                        _ => {
                            // Fallback for non-closure or top-level
                            return Err(Error::runtime(
                                span,
                                format!("cannot call value of type {}", callee.type_name(self.pool)),
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
                            return Err(Error::runtime(span, format!("expected bool, got {tname}")));
                        }
                    }
                }
                Op::JumpIfFalsePeek(target) => {
                    let cond = *self.stack.last().expect("stack underflow on JumpIfFalsePeek");
                    match cond {
                        RVal::Bool(true) => {
                            self.stack.pop();
                        }
                        RVal::Bool(false) => ip = target,
                        other => {
                            let span = chunk.spans[ip - 1];
                            let tname = other.type_name(self.pool);
                            return Err(Error::runtime(span, format!("expected bool, got {tname}")));
                        }
                    }
                }
                Op::JumpIfTruePeek(target) => {
                    let cond = *self.stack.last().expect("stack underflow on JumpIfTruePeek");
                    match cond {
                        RVal::Bool(false) => {
                            self.stack.pop();
                        }
                        RVal::Bool(true) => ip = target,
                        other => {
                            let span = chunk.spans[ip - 1];
                            let tname = other.type_name(self.pool);
                            return Err(Error::runtime(span, format!("expected bool, got {tname}")));
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
                Op::Import(idx) => {
                    let span = chunk.spans[ip - 1];
                    let name = &chunk.strings[idx as usize];
                    let Some(&mod_val) = self.modules.get(name.as_ref()) else {
                        return Err(Error::runtime(span, format!("unknown module '{name}'")));
                    };
                    self.pool.retain_val(mod_val);
                    self.define_top(name.to_string(), mod_val);
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
        debug_assert_eq!(self.stack.len(), frame_stack_start + 1);
        Ok(self.stack.pop().unwrap_or(RVal::Void))
    }
}

enum FrameSource<'a> {
    Top(&'a Chunk),
    Proto(Rc<FnProto>),
}

impl FrameSource<'_> {
    #[inline(always)]
    fn chunk(&self) -> &Chunk {
        match self {
            FrameSource::Top(c) => c,
            FrameSource::Proto(p) => &p.chunk,
        }
    }
}

impl Runner<'_, '_> {
    fn is_list(&self, v: RVal) -> bool {
        matches!(v, RVal::EmptyList)
            || matches!(v, RVal::Obj(id) if matches!(self.pool.get(id), Obj::Cons { .. }))
    }

    fn list_head(&self, v: RVal) -> Option<RVal> {
        match v {
            RVal::EmptyList => None,
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Cons { head, .. } => Some(*head),
                _ => None,
            },
            _ => None,
        }
    }

    fn list_tail(&self, v: RVal) -> RVal {
        match v {
            RVal::EmptyList => RVal::EmptyList,
            RVal::Obj(id) => match self.pool.get(id) {
                Obj::Cons { tail, .. } => *tail,
                _ => RVal::EmptyList,
            },
            _ => RVal::EmptyList,
        }
    }

    fn list_get(&self, mut v: RVal, mut idx: usize) -> Option<RVal> {
        while let RVal::Obj(id) = v {
            let Obj::Cons { head, tail, .. } = self.pool.get(id) else {
                break;
            };
            if idx == 0 {
                return Some(*head);
            }
            idx -= 1;
            v = *tail;
        }
        None
    }

    fn list_to_vec(&self, mut v: RVal) -> Vec<RVal> {
        let mut out = Vec::with_capacity(self.pool.list_len(v));
        while let RVal::Obj(id) = v {
            let Obj::Cons { head, tail, .. } = self.pool.get(id) else {
                break;
            };
            out.push(*head);
            v = *tail;
        }
        out
    }

    fn build_list_from_owned(&mut self, items: Vec<RVal>, span: Span) -> Result<RVal, Error> {
        let mut acc = RVal::EmptyList;
        for (idx, item) in items.iter().enumerate().rev() {
            match self.pool.alloc_cons(*item, acc, span) {
                Ok(next) => {
                    self.pool.release_val(acc);
                    acc = next;
                }
                Err(e) => {
                    self.pool.release_val(acc);
                    for &rem in &items[..=idx] {
                        self.pool.release_val(rem);
                    }
                    return Err(e);
                }
            }
            self.pool.release_val(*item);
        }
        Ok(acc)
    }

    fn build_list_from_owned_slice(&mut self, start: usize, span: Span) -> Result<RVal, Error> {
        let mut acc = RVal::EmptyList;
        let end = self.stack.len();
        for idx in (start..end).rev() {
            let item = self.stack[idx];
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
        }
        Ok(acc)
    }

    fn build_list_from_borrowed(&mut self, items: &[RVal], span: Span) -> Result<RVal, Error> {
        let mut acc = RVal::EmptyList;
        for &item in items.iter().rev() {
            match self.pool.alloc_cons(item, acc, span) {
                Ok(next) => {
                    self.pool.release_val(acc);
                    acc = next;
                }
                Err(e) => {
                    self.pool.release_val(acc);
                    return Err(e);
                }
            }
        }
        Ok(acc)
    }

    fn expect_list(&self, op: &str, v: RVal, span: Span) -> Result<(), Error> {
        if self.is_list(v) {
            Ok(())
        } else {
            Err(Error::runtime(
                span,
                format!("'{op}' expects list, got {}", v.type_name(self.pool)),
            ))
        }
    }

    fn expect_one_list(&self, op: &str, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [v] = args else {
            return Err(Error::runtime(span, format!("'{op}' takes 1 argument")));
        };
        self.expect_list(op, *v, span)?;
        Ok(*v)
    }

    fn expect_int(&self, op: &str, v: RVal, span: Span) -> Result<i64, Error> {
        match v {
            RVal::Int(n) => Ok(n),
            other => Err(Error::runtime(
                span,
                format!("'{op}' expects i64, got {}", other.type_name(self.pool)),
            )),
        }
    }

    fn apply_builtin(&mut self, b: BuiltinId, args: &[RVal], span: Span) -> Result<RVal, Error> {
        match b {
            BuiltinId::Add
            | BuiltinId::Sub
            | BuiltinId::Mul
            | BuiltinId::Div
            | BuiltinId::Rem => self.arith(b, args, span),
            BuiltinId::Eq | BuiltinId::Ne => self.eq_cmp(b == BuiltinId::Eq, args, span),
            BuiltinId::Lt | BuiltinId::Le | BuiltinId::Gt | BuiltinId::Ge => {
                self.ord_cmp(b, args, span)
            }
            BuiltinId::Not => {
                let [v] = args else {
                    return Err(Error::runtime(span, "'not' takes 1 argument"));
                };
                match v {
                    RVal::Bool(b) => Ok(RVal::Bool(!b)),
                    other => Err(Error::runtime(
                        span,
                        format!("'not' expects bool, got {}", other.type_name(self.pool)),
                    )),
                }
            }
            BuiltinId::AsF64 => {
                let [v] = args else {
                    return Err(Error::runtime(span, "'as_f64' takes 1 argument"));
                };
                match v {
                    RVal::Int(n) => Ok(RVal::Float(*n as f64)),
                    RVal::Float(x) => Ok(RVal::Float(*x)),
                    other => Err(Error::runtime(
                        span,
                        format!("'as_f64' expects number, got {}", other.type_name(self.pool)),
                    )),
                }
            }
            BuiltinId::AsI64 => {
                let [v] = args else {
                    return Err(Error::runtime(span, "'as_i64' takes 1 argument"));
                };
                match v {
                    RVal::Int(n) => Ok(RVal::Int(*n)),
                    RVal::Float(x) => {
                        if !x.is_finite() || *x < (i64::MIN as f64) || *x > (i64::MAX as f64) {
                            return Err(Error::runtime(span, "float out of i64 range"));
                        }
                        Ok(RVal::Int(*x as i64))
                    }
                    other => Err(Error::runtime(
                        span,
                        format!("'as_i64' expects number, got {}", other.type_name(self.pool)),
                    )),
                }
            }
            BuiltinId::Sin
            | BuiltinId::Cos
            | BuiltinId::Sqrt
            | BuiltinId::Abs
            | BuiltinId::Floor
            | BuiltinId::Ceil => self.math(b, args, span),
            BuiltinId::List => self.build_list_from_borrowed(args, span),
            BuiltinId::Len => {
                let [v] = args else {
                    return Err(Error::runtime(span, "'len' takes 1 argument"));
                };
                if self.is_list(*v) {
                    return Ok(RVal::Int(self.pool.list_len(*v) as i64));
                }
                if let RVal::Obj(id) = v {
                    match self.pool.get(*id) {
                        Obj::Array(arr) => return Ok(RVal::Int(arr.len() as i64)),
                        Obj::Str(s) => return Ok(RVal::Int(s.chars().count() as i64)),
                        _ => {}
                    }
                }
                Err(Error::runtime(
                    span,
                    format!(
                        "'len' expects list, array, or string, got {}",
                        v.type_name(self.pool)
                    ),
                ))
            }
            BuiltinId::IsEmpty => {
                let xs = self.expect_one_list("is_empty", args, span)?;
                Ok(RVal::Bool(self.pool.list_len(xs) == 0))
            }
            BuiltinId::Get => {
                let [list_v, idx_v] = args else {
                    return Err(Error::runtime(span, "'get' takes 2 arguments (seq, index)"));
                };
                let idx = self.expect_int("get", *idx_v, span)?;
                if let RVal::Obj(id) = *list_v
                    && let Obj::Array(arr) = self.pool.get(id)
                {
                    if idx < 0 || (idx as usize) >= arr.len() {
                        return Ok(RVal::None);
                    }
                    let item = arr[idx as usize];
                    self.pool.retain_val(item);
                    return match self.pool.alloc_obj(Obj::Some(item), span) {
                        Ok(sid) => Ok(RVal::Obj(sid)),
                        Err(e) => {
                            self.pool.release_val(item);
                            Err(e)
                        }
                    };
                }
                self.expect_list("get", *list_v, span)?;
                if idx < 0 {
                    return Ok(RVal::None);
                }
                match self.list_get(*list_v, idx as usize) {
                    Some(item) => {
                        self.pool.retain_val(item);
                        match self.pool.alloc_obj(Obj::Some(item), span) {
                            Ok(id) => Ok(RVal::Obj(id)),
                            Err(e) => {
                                self.pool.release_val(item);
                                Err(e)
                            }
                        }
                    }
                    None => Ok(RVal::None),
                }
            }
            BuiltinId::Car => {
                let xs = self.expect_one_list("car", args, span)?;
                match self.list_head(xs) {
                    Some(head) => {
                        self.pool.retain_val(head);
                        match self.pool.alloc_obj(Obj::Some(head), span) {
                            Ok(id) => Ok(RVal::Obj(id)),
                            Err(e) => {
                                self.pool.release_val(head);
                                Err(e)
                            }
                        }
                    }
                    None => Ok(RVal::None),
                }
            }
            BuiltinId::Cdr => {
                let xs = self.expect_one_list("cdr", args, span)?;
                let tail = self.list_tail(xs);
                self.pool.retain_val(tail);
                Ok(tail)
            }
            BuiltinId::Last => {
                let xs = self.expect_one_list("last", args, span)?;
                let items = self.list_to_vec(xs);
                match items.last().copied() {
                    Some(v) => {
                        self.pool.retain_val(v);
                        match self.pool.alloc_obj(Obj::Some(v), span) {
                            Ok(id) => Ok(RVal::Obj(id)),
                            Err(e) => {
                                self.pool.release_val(v);
                                Err(e)
                            }
                        }
                    }
                    None => Ok(RVal::None),
                }
            }
            BuiltinId::Cons => {
                let [item, list_v] = args else {
                    return Err(Error::runtime(span, "'cons' takes 2 arguments (item, list)"));
                };
                self.expect_list("cons", *list_v, span)?;
                self.pool.alloc_cons(*item, *list_v, span)
            }
            BuiltinId::Push => {
                let [list_v, item] = args else {
                    return Err(Error::runtime(span, "'push' takes 2 arguments (list, item)"));
                };
                self.expect_list("push", *list_v, span)?;
                let mut items = self.list_to_vec(*list_v);
                items.push(*item);
                self.build_list_from_borrowed(&items, span)
            }
            BuiltinId::Append => {
                let mut items = Vec::new();
                for &arg in args {
                    self.expect_list("append", arg, span)?;
                    items.extend(self.list_to_vec(arg));
                }
                self.build_list_from_borrowed(&items, span)
            }
            BuiltinId::Flatten => {
                let xs = self.expect_one_list("flatten", args, span)?;
                let outer = self.list_to_vec(xs);
                let mut items = Vec::new();
                for sub in outer {
                    self.expect_list("flatten", sub, span)?;
                    items.extend(self.list_to_vec(sub));
                }
                self.build_list_from_borrowed(&items, span)
            }
            BuiltinId::Slice => {
                let [list_v, start_v, end_v] = args else {
                    return Err(Error::runtime(
                        span,
                        "'slice' takes 3 arguments (list, start, end)",
                    ));
                };
                self.expect_list("slice", *list_v, span)?;
                let start = self.expect_int("slice", *start_v, span)?;
                let end = self.expect_int("slice", *end_v, span)?;
                let len = self.pool.list_len(*list_v) as i64;
                if start < 0 || end < start || end > len {
                    return Err(Error::runtime(
                        span,
                        format!("slice range {start}..{end} out of range (length {len})"),
                    ));
                }
                let items = self.list_to_vec(*list_v);
                self.build_list_from_borrowed(&items[start as usize..end as usize], span)
            }
            BuiltinId::Take => {
                let [list_v, n_v] = args else {
                    return Err(Error::runtime(span, "'take' takes 2 arguments (list, n)"));
                };
                self.expect_list("take", *list_v, span)?;
                let n = self.expect_int("take", *n_v, span)?;
                if n < 0 {
                    return Err(Error::runtime(span, "'take' count cannot be negative"));
                }
                let items = self.list_to_vec(*list_v);
                let end = (n as usize).min(items.len());
                self.build_list_from_borrowed(&items[..end], span)
            }
            BuiltinId::Drop => {
                let [list_v, n_v] = args else {
                    return Err(Error::runtime(span, "'drop' takes 2 arguments (list, n)"));
                };
                self.expect_list("drop", *list_v, span)?;
                let mut n = self.expect_int("drop", *n_v, span)?;
                if n < 0 {
                    return Err(Error::runtime(span, "'drop' count cannot be negative"));
                }
                let mut cur = *list_v;
                while n > 0 && self.pool.list_len(cur) > 0 {
                    cur = self.list_tail(cur);
                    n -= 1;
                }
                self.pool.retain_val(cur);
                Ok(cur)
            }
            BuiltinId::Reverse => {
                let xs = self.expect_one_list("reverse", args, span)?;
                let mut acc = RVal::EmptyList;
                let mut cur = xs;
                while let Some(head) = self.list_head(cur) {
                    match self.pool.alloc_cons(head, acc, span) {
                        Ok(next) => {
                            self.pool.release_val(acc);
                            acc = next;
                        }
                        Err(e) => {
                            self.pool.release_val(acc);
                            return Err(e);
                        }
                    }
                    cur = self.list_tail(cur);
                }
                Ok(acc)
            }
            BuiltinId::Range => {
                let [start_v, end_v] = args else {
                    return Err(Error::runtime(span, "'range' takes 2 arguments (start, end)"));
                };
                let start = self.expect_int("range", *start_v, span)?;
                let end = self.expect_int("range", *end_v, span)?;
                if end < start {
                    return Ok(RVal::EmptyList);
                }
                let mut acc = RVal::EmptyList;
                for n in (start..end).rev() {
                    self.steps += 1;
                    if self.steps > self.limits.steps {
                        self.pool.release_val(acc);
                        return Err(self.step_limit_err(span));
                    }
                    match self.pool.alloc_cons(RVal::Int(n), acc, span) {
                        Ok(next) => {
                            self.pool.release_val(acc);
                            acc = next;
                        }
                        Err(e) => {
                            self.pool.release_val(acc);
                            return Err(e);
                        }
                    }
                }
                Ok(acc)
            }
            BuiltinId::Contains => {
                let [list_v, target] = args else {
                    return Err(Error::runtime(
                        span,
                        "'contains' takes 2 arguments (list, value)",
                    ));
                };
                self.expect_list("contains", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                for x in items {
                    if x.type_name(self.pool) != target.type_name(self.pool) {
                        return Err(Error::runtime(
                            span,
                            format!(
                                "cannot compare {} and {}",
                                x.type_name(self.pool),
                                target.type_name(self.pool)
                            ),
                        ));
                    }
                    if self.values_equal(x, *target) {
                        return Ok(RVal::Bool(true));
                    }
                }
                Ok(RVal::Bool(false))
            }
            BuiltinId::Find => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(span, "'find' takes 2 arguments (fn, list)"));
                };
                self.expect_list("find", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                for x in items {
                    let res = self.apply(*f, &[x], span)?;
                    match res {
                        RVal::Bool(true) => {
                            self.pool.retain_val(x);
                            return match self.pool.alloc_obj(Obj::Some(x), span) {
                                Ok(id) => Ok(RVal::Obj(id)),
                                Err(e) => {
                                    self.pool.release_val(x);
                                    Err(e)
                                }
                            };
                        }
                        RVal::Bool(false) => {}
                        other => {
                            let tname = other.type_name(self.pool);
                            self.pool.release_val(res);
                            return Err(Error::runtime(
                                span,
                                format!("'find' predicate must return bool, got {tname}"),
                            ));
                        }
                    }
                }
                Ok(RVal::None)
            }
            BuiltinId::Any => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(span, "'any' takes 2 arguments (fn, list)"));
                };
                self.expect_list("any", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                for x in items {
                    let res = self.apply(*f, &[x], span)?;
                    match res {
                        RVal::Bool(true) => return Ok(RVal::Bool(true)),
                        RVal::Bool(false) => {}
                        other => {
                            let tname = other.type_name(self.pool);
                            self.pool.release_val(res);
                            return Err(Error::runtime(
                                span,
                                format!("'any' predicate must return bool, got {tname}"),
                            ));
                        }
                    }
                }
                Ok(RVal::Bool(false))
            }
            BuiltinId::All => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(span, "'all' takes 2 arguments (fn, list)"));
                };
                self.expect_list("all", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                for x in items {
                    let res = self.apply(*f, &[x], span)?;
                    match res {
                        RVal::Bool(true) => {}
                        RVal::Bool(false) => return Ok(RVal::Bool(false)),
                        other => {
                            let tname = other.type_name(self.pool);
                            self.pool.release_val(res);
                            return Err(Error::runtime(
                                span,
                                format!("'all' predicate must return bool, got {tname}"),
                            ));
                        }
                    }
                }
                Ok(RVal::Bool(true))
            }
            BuiltinId::Map => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(span, "'map' takes 2 arguments (fn, list)"));
                };
                self.expect_list("map", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                let mut out = Vec::with_capacity(items.len());
                for x in items {
                    match self.apply(*f, &[x], span) {
                        Ok(v) => out.push(v),
                        Err(e) => {
                            self.release_all(out);
                            return Err(e);
                        }
                    }
                }
                self.build_list_from_owned(out, span)
            }
            BuiltinId::Filter => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(span, "'filter' takes 2 arguments (fn, list)"));
                };
                self.expect_list("filter", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                let mut out = Vec::new();
                for x in items {
                    let res = match self.apply(*f, &[x], span) {
                        Ok(v) => v,
                        Err(e) => {
                            self.release_all(out);
                            return Err(e);
                        }
                    };
                    match res {
                        RVal::Bool(true) => {
                            self.pool.retain_val(x);
                            out.push(x);
                        }
                        RVal::Bool(false) => {}
                        other => {
                            let tname = other.type_name(self.pool);
                            self.pool.release_val(res);
                            self.release_all(out);
                            return Err(Error::runtime(
                                span,
                                format!("'filter' predicate must return bool, got {tname}"),
                            ));
                        }
                    }
                }
                self.build_list_from_owned(out, span)
            }
            BuiltinId::FlatMap => {
                let [f, list_v] = args else {
                    return Err(Error::runtime(
                        span,
                        "'flat_map' takes 2 arguments (fn, list)",
                    ));
                };
                self.expect_list("flat_map", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                let mut out = Vec::new();
                for x in items {
                    let mapped = match self.apply(*f, &[x], span) {
                        Ok(v) => v,
                        Err(e) => {
                            self.release_all(out);
                            return Err(e);
                        }
                    };
                    if let Err(e) = self.expect_list("flat_map", mapped, span) {
                        self.pool.release_val(mapped);
                        self.release_all(out);
                        return Err(e);
                    }
                    for sub in self.list_to_vec(mapped) {
                        self.pool.retain_val(sub);
                        out.push(sub);
                    }
                    self.pool.release_val(mapped);
                }
                self.build_list_from_owned(out, span)
            }
            BuiltinId::Fold => {
                let [f, init, list_v] = args else {
                    return Err(Error::runtime(
                        span,
                        "'fold' takes 3 arguments (fn, init, list)",
                    ));
                };
                self.expect_list("fold", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                let mut acc = *init;
                self.pool.retain_val(acc);
                for x in items {
                    match self.apply(*f, &[acc, x], span) {
                        Ok(next) => {
                            self.pool.release_val(acc);
                            acc = next;
                        }
                        Err(e) => {
                            self.pool.release_val(acc);
                            return Err(e);
                        }
                    }
                }
                Ok(acc)
            }
            BuiltinId::Zip => {
                let [a_v, b_v] = args else {
                    return Err(Error::runtime(span, "'zip' takes 2 arguments (list, list)"));
                };
                self.expect_list("zip", *a_v, span)?;
                self.expect_list("zip", *b_v, span)?;
                let a = self.list_to_vec(*a_v);
                let b = self.list_to_vec(*b_v);
                let mut out = Vec::with_capacity(a.len().min(b.len()));
                for (x, y) in a.into_iter().zip(b.into_iter()) {
                    self.pool.retain_val(x);
                    self.pool.retain_val(y);
                    match self.pool.alloc_obj(Obj::Tuple(vec![x, y]), span) {
                        Ok(id) => out.push(RVal::Obj(id)),
                        Err(e) => {
                            self.pool.release_val(x);
                            self.pool.release_val(y);
                            self.release_all(out);
                            return Err(e);
                        }
                    }
                }
                self.build_list_from_owned(out, span)
            }
            BuiltinId::Enumerate => {
                let xs = self.expect_one_list("enumerate", args, span)?;
                let items = self.list_to_vec(xs);
                let mut out = Vec::with_capacity(items.len());
                for (i, x) in items.into_iter().enumerate() {
                    self.pool.retain_val(x);
                    match self
                        .pool
                        .alloc_obj(Obj::Tuple(vec![RVal::Int(i as i64), x]), span)
                    {
                        Ok(id) => out.push(RVal::Obj(id)),
                        Err(e) => {
                            self.pool.release_val(x);
                            self.release_all(out);
                            return Err(e);
                        }
                    }
                }
                self.build_list_from_owned(out, span)
            }
            BuiltinId::Sort => {
                let xs = self.expect_one_list("sort", args, span)?;
                let mut items = self.list_to_vec(xs);
                let mut sort_err = None;
                items.sort_by(|&a, &b| match self.compare_vals(a, b, span) {
                    Ok(ord) => ord,
                    Err(e) => {
                        if sort_err.is_none() {
                            sort_err = Some(e);
                        }
                        std::cmp::Ordering::Equal
                    }
                });
                if let Some(e) = sort_err {
                    return Err(e);
                }
                self.build_list_from_borrowed(&items, span)
            }
            BuiltinId::SortBy => {
                let [key_fn, list_v] = args else {
                    return Err(Error::runtime(
                        span,
                        "'sort_by' takes 2 arguments (key_fn, list)",
                    ));
                };
                self.expect_list("sort_by", *list_v, span)?;
                let items = self.list_to_vec(*list_v);
                let mut keyed = Vec::with_capacity(items.len());
                for x in items {
                    match self.apply(*key_fn, &[x], span) {
                        Ok(k) => keyed.push((k, x)),
                        Err(e) => {
                            for (k, _) in keyed {
                                self.pool.release_val(k);
                            }
                            return Err(e);
                        }
                    }
                }
                let mut sort_err = None;
                keyed.sort_by(|(ka, _), (kb, _)| match self.compare_vals(*ka, *kb, span) {
                    Ok(ord) => ord,
                    Err(e) => {
                        if sort_err.is_none() {
                            sort_err = Some(e);
                        }
                        std::cmp::Ordering::Equal
                    }
                });
                let sorted: Vec<RVal> = keyed.iter().map(|(_, v)| *v).collect();
                for (k, _) in keyed {
                    self.pool.release_val(k);
                }
                if let Some(e) = sort_err {
                    return Err(e);
                }
                self.build_list_from_borrowed(&sorted, span)
            }
            BuiltinId::Unique => {
                let xs = self.expect_one_list("unique", args, span)?;
                let items = self.list_to_vec(xs);
                let mut out: Vec<RVal> = Vec::new();
                for x in items {
                    if !out.iter().any(|&seen| self.values_equal(seen, x)) {
                        out.push(x);
                    }
                }
                self.build_list_from_borrowed(&out, span)
            }
            BuiltinId::ListToArray => {
                let xs = self.expect_one_list("list->array", args, span)?;
                let items = self.list_to_vec(xs);
                for &item in &items {
                    self.pool.retain_val(item);
                }
                Ok(RVal::Obj(self.pool.alloc_obj(Obj::Array(items), span)?))
            }
            BuiltinId::ArrayToList => {
                let [v] = args else {
                    return Err(Error::runtime(span, "'array->list' takes 1 argument"));
                };
                let items = match v {
                    RVal::Obj(id) => match self.pool.get(*id) {
                        Obj::Array(arr) => arr.clone(),
                        _ => {
                            return Err(Error::runtime(
                                span,
                                format!("'array->list' expects array, got {}", v.type_name(self.pool)),
                            ));
                        }
                    },
                    other => {
                        return Err(Error::runtime(
                            span,
                            format!(
                                "'array->list' expects array, got {}",
                                other.type_name(self.pool)
                            ),
                        ));
                    }
                };
                self.build_list_from_borrowed(&items, span)
            }
        }
    }

    fn values_equal(&self, a: RVal, b: RVal) -> bool {
        match (a, b) {
            (RVal::Void, RVal::Void) => true,
            (RVal::Int(x), RVal::Int(y)) => x == y,
            (RVal::Float(x), RVal::Float(y)) => x == y,
            (RVal::Bool(x), RVal::Bool(y)) => x == y,
            (RVal::Char(x), RVal::Char(y)) => x == y,
            (RVal::None, RVal::None) => true,
            (RVal::EmptyList, RVal::EmptyList) => true,
            (RVal::Builtin(x), RVal::Builtin(y)) => x == y,
            (RVal::Obj(id1), RVal::Obj(id2)) => {
                if id1 == id2 {
                    return true;
                }
                match (self.pool.get(id1), self.pool.get(id2)) {
                    (
                        Obj::Cons {
                            head: h1,
                            tail: t1,
                            len: l1,
                        },
                        Obj::Cons {
                            head: h2,
                            tail: t2,
                            len: l2,
                        },
                    ) => l1 == l2 && self.values_equal(*h1, *h2) && self.values_equal(*t1, *t2),
                    (Obj::Str(s1), Obj::Str(s2)) => s1 == s2,
                    (Obj::Array(xs1), Obj::Array(xs2)) | (Obj::Tuple(xs1), Obj::Tuple(xs2)) => {
                        xs1.len() == xs2.len()
                            && xs1
                                .iter()
                                .zip(xs2.iter())
                                .all(|(x, y)| self.values_equal(*x, *y))
                    }
                    (
                        Obj::Struct {
                            name: n1,
                            fields: f1,
                        },
                        Obj::Struct {
                            name: n2,
                            fields: f2,
                        },
                    ) => {
                        n1 == n2
                            && f1.len() == f2.len()
                            && f1.iter().all(|(k, v1)| {
                                f2.get(k).is_some_and(|v2| self.values_equal(*v1, *v2))
                            })
                    }
                    (
                        Obj::Variant {
                            type_name: t1,
                            variant: v1,
                            data: d1,
                        },
                        Obj::Variant {
                            type_name: t2,
                            variant: v2,
                            data: d2,
                        },
                    ) => {
                        t1 == t2
                            && v1 == v2
                            && match (d1, d2) {
                                (None, None) => true,
                                (Some(x), Some(y)) => self.values_equal(*x, *y),
                                _ => false,
                            }
                    }
                    (Obj::Some(x), Obj::Some(y))
                    | (Obj::Ok(x), Obj::Ok(y))
                    | (Obj::Err(x), Obj::Err(y)) => self.values_equal(*x, *y),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn compare_vals(&self, a: RVal, b: RVal, span: Span) -> Result<std::cmp::Ordering, Error> {
        match (a, b) {
            (RVal::Int(x), RVal::Int(y)) => Ok(x.cmp(&y)),
            (RVal::Float(x), RVal::Float(y)) => x
                .partial_cmp(&y)
                .ok_or_else(|| Error::runtime(span, "cannot order NaN values")),
            (RVal::Char(x), RVal::Char(y)) => Ok(x.cmp(&y)),
            (RVal::Obj(id1), RVal::Obj(id2)) => match (self.pool.get(id1), self.pool.get(id2)) {
                (Obj::Str(s1), Obj::Str(s2)) => Ok(s1.cmp(s2)),
                _ => Err(Error::runtime(
                    span,
                    format!(
                        "cannot order {} and {}",
                        a.type_name(self.pool),
                        b.type_name(self.pool)
                    ),
                )),
            },
            _ => Err(Error::runtime(
                span,
                format!(
                    "cannot order {} and {}",
                    a.type_name(self.pool),
                    b.type_name(self.pool)
                ),
            )),
        }
    }

    fn arith(&self, op: BuiltinId, args: &[RVal], span: Span) -> Result<RVal, Error> {
        if op == BuiltinId::Sub && args.len() == 1 {
            return match args[0] {
                RVal::Int(a) => a
                    .checked_neg()
                    .map(RVal::Int)
                    .ok_or_else(|| Error::runtime(span, "i64 overflow")),
                RVal::Float(a) => Ok(RVal::Float(-a)),
                other => Err(Error::runtime(
                    span,
                    format!("cannot negate {}", other.type_name(self.pool)),
                )),
            };
        }
        if (op == BuiltinId::Add || op == BuiltinId::Mul) && args.len() >= 2 {
            let mut acc = args[0];
            for &rhs in &args[1..] {
                acc = self.arith_bin(op, acc, rhs, span)?;
            }
            return Ok(acc);
        }
        let [a, b] = args else {
            return Err(Error::runtime(
                span,
                format!("'{}' takes 2 arguments", op.name()),
            ));
        };
        self.arith_bin(op, *a, *b, span)
    }

    fn arith_bin(&self, op: BuiltinId, a: RVal, b: RVal, span: Span) -> Result<RVal, Error> {
        match (a, b) {
            (RVal::Int(x), RVal::Int(y)) => {
                let res = match op {
                    BuiltinId::Add => x.checked_add(y),
                    BuiltinId::Sub => x.checked_sub(y),
                    BuiltinId::Mul => x.checked_mul(y),
                    BuiltinId::Div => {
                        if y == 0 {
                            return Err(Error::runtime(span, "division by zero"));
                        }
                        x.checked_div(y)
                    }
                    BuiltinId::Rem => {
                        if y == 0 {
                            return Err(Error::runtime(span, "division by zero"));
                        }
                        x.checked_rem(y)
                    }
                    _ => unreachable!(),
                };
                res.map(RVal::Int)
                    .ok_or_else(|| Error::runtime(span, "i64 overflow"))
            }
            (RVal::Float(x), RVal::Float(y)) => match op {
                BuiltinId::Add => Ok(RVal::Float(x + y)),
                BuiltinId::Sub => Ok(RVal::Float(x - y)),
                BuiltinId::Mul => Ok(RVal::Float(x * y)),
                BuiltinId::Div => Ok(RVal::Float(x / y)),
                BuiltinId::Rem => Ok(RVal::Float(x % y)),
                _ => unreachable!(),
            },
            _ => Err(Error::runtime(
                span,
                format!(
                    "cannot apply '{}' to {} and {} (no implicit conversion)",
                    op.name(),
                    a.type_name(self.pool),
                    b.type_name(self.pool)
                ),
            )),
        }
    }

    fn eq_cmp(&self, want_eq: bool, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [a, b] = args else {
            return Err(Error::runtime(span, "comparison takes 2 arguments"));
        };
        if a.type_name(self.pool) != b.type_name(self.pool) {
            return Err(Error::runtime(
                span,
                format!(
                    "cannot compare {} and {}",
                    a.type_name(self.pool),
                    b.type_name(self.pool)
                ),
            ));
        }
        let eq = self.values_equal(*a, *b);
        Ok(RVal::Bool(if want_eq { eq } else { !eq }))
    }

    fn ord_cmp(&self, op: BuiltinId, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [a, b] = args else {
            return Err(Error::runtime(
                span,
                format!("'{}' takes 2 arguments", op.name()),
            ));
        };
        let res = match (*a, *b) {
            (RVal::Int(x), RVal::Int(y)) => match op {
                BuiltinId::Lt => x < y,
                BuiltinId::Le => x <= y,
                BuiltinId::Gt => x > y,
                BuiltinId::Ge => x >= y,
                _ => unreachable!(),
            },
            (RVal::Float(x), RVal::Float(y)) => match op {
                BuiltinId::Lt => x < y,
                BuiltinId::Le => x <= y,
                BuiltinId::Gt => x > y,
                BuiltinId::Ge => x >= y,
                _ => unreachable!(),
            },
            _ => {
                return Err(Error::runtime(
                    span,
                    format!(
                        "cannot compare {} and {}",
                        a.type_name(self.pool),
                        b.type_name(self.pool)
                    ),
                ));
            }
        };
        Ok(RVal::Bool(res))
    }

    fn math(&self, b: BuiltinId, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [v] = args else {
            return Err(Error::runtime(
                span,
                format!("'{}' takes 1 argument", b.name()),
            ));
        };
        if b == BuiltinId::Abs {
            return match v {
                RVal::Int(n) => n
                    .checked_abs()
                    .map(RVal::Int)
                    .ok_or_else(|| Error::runtime(span, "i64 overflow")),
                RVal::Float(x) => Ok(RVal::Float(x.abs())),
                other => Err(Error::runtime(
                    span,
                    format!("'abs' expects number, got {}", other.type_name(self.pool)),
                )),
            };
        }
        let RVal::Float(x) = v else {
            return Err(Error::runtime(
                span,
                format!("'{}' expects f64, got {}", b.name(), v.type_name(self.pool)),
            ));
        };
        let res = match b {
            BuiltinId::Sin => x.sin(),
            BuiltinId::Cos => x.cos(),
            BuiltinId::Sqrt => x.sqrt(),
            BuiltinId::Floor => x.floor(),
            BuiltinId::Ceil => x.ceil(),
            _ => unreachable!(),
        };
        Ok(RVal::Float(res))
    }

    fn match_pattern(&self, pat: &Pattern, val: RVal) -> Option<Option<RVal>> {
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
                } if type_name == vt && variant == vv => match (binding, data) {
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
