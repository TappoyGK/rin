//! バイトコード VM の実行環境 `Machine` と `Runner` を定義する。

mod builtins;
mod ops;
mod vm;

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::alloc::{Allocator, HeapAllocator};
use crate::bytecode::{BuiltinId, Capture, Chunk, FnProto, SymTable};
use crate::compiler::compile_program;
use crate::error::{Error, ErrorKind, Limits, Span};
use crate::parser::{ParseScratch, ParseScratchGuard};
use crate::pool::{Obj, Pool, RVal};
use crate::value::Value;

/// 既定の `HeapAllocator` を用いて Rin ソースを評価する。
pub fn eval_with(source: &str, bindings: &[(&str, Value)], limits: Limits) -> Result<Value, Error> {
    let mut alloc = HeapAllocator;
    eval_with_allocator(source, bindings, limits, &mut alloc)
}

/// 呼び出し側が指定した `Allocator` を用いて Rin ソースを評価する。
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

/// 呼び出し側の `Allocator` 上で動作する永続的な Rin 実行環境。
///
/// トップレベル定義（`def` / `defn`）は `eval` 呼び出しをまたいで保持され、
/// 評価中に作られた一時値は参照カウントとフリーリストで即座に再利用する。
pub struct Machine<'a> {
    limits: Limits,
    pool: Pool<'a>,
    syms: SymTable,
    globals: Vec<Option<RVal>>,
    modules: BTreeMap<String, RVal>,
    module_ids: BTreeMap<String, u32>,
    module_names: Vec<String>,
    module_envs: Vec<Vec<Option<RVal>>>,
    parse_scratch: ParseScratch,
}

fn set_slot(slots: &mut Vec<Option<RVal>>, id: u32, val: RVal) -> Option<RVal> {
    let idx = id as usize;
    if idx >= slots.len() {
        slots.resize(idx + 1, None);
    }
    slots[idx].replace(val)
}

impl<'a> Machine<'a> {
    pub fn new(limits: Limits, alloc: &'a mut dyn Allocator) -> Self {
        let mut syms = SymTable::default();
        let mut globals = Vec::new();
        for &b in BuiltinId::ALL {
            let id = syms.intern(b.name());
            set_slot(&mut globals, id, RVal::Builtin(b));
        }
        let pi_id = syms.intern("pi");
        set_slot(&mut globals, pi_id, RVal::Float(std::f64::consts::PI));
        Self {
            limits,
            pool: Pool::new(alloc, limits.memory_bytes),
            syms,
            globals,
            modules: BTreeMap::new(),
            module_ids: BTreeMap::new(),
            module_names: Vec::new(),
            module_envs: Vec::new(),
            parse_scratch: ParseScratch::new(),
        }
    }

    pub fn bind(&mut self, name: &str, value: &Value) -> Result<(), Error> {
        let rval = self.pool.import(value, Span::default())?;
        let id = self.syms.intern(name);
        if let Some(prev) = set_slot(&mut self.globals, id, rval) {
            self.pool.release_val(prev);
        }
        Ok(())
    }

    pub fn live_bytes(&self) -> usize {
        self.pool.live_bytes()
    }

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
            compile_program(exprs, Some(mod_id), &mut self.syms).map_err(with_mod)?
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

    pub fn eval(&mut self, source: &str) -> Result<Value, Error> {
        let chunk = {
            let mut scratch = ParseScratchGuard::new(&mut self.parse_scratch);
            let exprs = scratch.parse(source)?;
            compile_program(exprs, None, &mut self.syms)?
        };
        let mut runner = self.runner(None);
        let last = runner.run_top_chunk(&chunk)?;
        let exported = runner.pool.export(last);
        runner.pool.release_val(last);
        Ok(exported)
    }

    /// 定義済みの大域関数をソース再解析なしで直接呼び出す。
    pub fn call(&mut self, fn_name: &str, args: &[Value]) -> Result<Value, Error> {
        let Some(callee) = self
            .syms
            .get(fn_name)
            .and_then(|id| self.globals.get(id as usize).copied().flatten())
        else {
            return Err(Error::runtime(
                "expr.name",
                Span::default(),
                format!("unknown function '{fn_name}'"),
            ));
        };
        self.call_rval(callee, args)
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
                format!("unknown module '{mod_name}'"),
            ));
        };
        let Obj::Module { exports, .. } = self.pool.get(mod_id) else {
            unreachable!();
        };
        let Some(&callee) = exports.get(fn_name) else {
            return Err(Error::runtime(
                "expr.module",
                Span::default(),
                format!("module '{mod_name}' has no export '{fn_name}'"),
            ));
        };
        self.call_rval(callee, args)
    }

    pub(super) fn call_rval(&mut self, callee: RVal, args: &[Value]) -> Result<Value, Error> {
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
        let mut runner = self.runner(None);
        let res = runner.apply(callee, &r_args, Span::default(), CallOrigin::Host);
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
        for env in old_mod_envs {
            for v in env.into_iter().flatten() {
                self.pool.release_val(v);
            }
        }
        let old = std::mem::take(&mut self.globals);
        for v in old.into_iter().flatten() {
            self.pool.release_val(v);
        }
    }
}

impl<'a> Machine<'a> {
    fn runner(&mut self, active_module: Option<u32>) -> Runner<'_, 'a> {
        Runner {
            limits: self.limits,
            steps: 0,
            depth: 0,
            pool: &mut self.pool,
            syms: &self.syms,
            globals: &mut self.globals,
            modules: &self.modules,
            module_names: &self.module_names,
            module_envs: &mut self.module_envs,
            active_module,
            export_list: None,
            stack: Vec::with_capacity(64),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum CallOrigin<'a> {
    Rin(Span),
    Host,
    Builtin { name: &'a str, at: Span },
}

pub(super) struct Runner<'a, 'p> {
    pub(super) limits: Limits,
    pub(super) steps: usize,
    pub(super) depth: usize,
    pub(super) pool: &'a mut Pool<'p>,
    pub(super) syms: &'a SymTable,
    pub(super) globals: &'a mut Vec<Option<RVal>>,
    pub(super) modules: &'a BTreeMap<String, RVal>,
    pub(super) module_names: &'a [String],
    pub(super) module_envs: &'a mut Vec<Vec<Option<RVal>>>,
    pub(super) active_module: Option<u32>,
    pub(super) export_list: Option<Vec<String>>,
    pub(super) stack: Vec<RVal>,
}

impl<'a, 'p> Runner<'a, 'p> {
    #[inline(always)]
    pub(super) fn tick(&mut self, chunk: &Chunk, start: u32, count: u32) -> Result<(), Error> {
        let next = self.steps + count as usize;
        if next > self.limits.steps {
            let over = self.limits.steps.saturating_sub(self.steps);
            let span = chunk.tick_spans[start as usize + over];
            self.steps = next;
            return Err(self.step_limit_err(span));
        }
        self.steps = next;
        Ok(())
    }

    #[cold]
    pub(super) fn step_limit_err(&self, span: Span) -> Error {
        Error::new(
            "eval.limits",
            ErrorKind::TooManySteps,
            span,
            format!("step limit exceeded ({})", self.limits.steps),
        )
    }

    pub(super) fn enter_depth(&mut self, span: Span) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > self.limits.depth {
            return Err(self.depth_limit_err(span));
        }
        Ok(())
    }

    #[cold]
    pub(super) fn depth_limit_err(&self, span: Span) -> Error {
        Error::new(
            "eval.limits",
            ErrorKind::TooDeep,
            span,
            format!("call depth limit exceeded ({})", self.limits.depth),
        )
    }

    pub(super) fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub(super) fn lookup_global(&mut self, id: u32, span: Span) -> Result<RVal, Error> {
        let idx = id as usize;
        if let Some(mod_id) = self.active_module
            && let Some(Some(v)) = self.module_envs[mod_id as usize].get(idx).copied()
        {
            self.pool.retain_val(v);
            return Ok(v);
        }
        if let Some(Some(v)) = self.globals.get(idx).copied() {
            self.pool.retain_val(v);
            return Ok(v);
        }
        let name = self.syms.names.get(idx).map(|s| s.as_ref()).unwrap_or("?");
        Err(Error::runtime(
            "expr.name",
            span,
            format!("unknown name '{name}'"),
        ))
    }

    pub(super) fn define_top(&mut self, id: u32, val: RVal) {
        let prev = if let Some(mod_id) = self.active_module {
            set_slot(&mut self.module_envs[mod_id as usize], id, val)
        } else {
            set_slot(self.globals, id, val)
        };
        if let Some(p) = prev {
            self.pool.release_val(p);
        }
    }

    pub(super) fn release_all(&mut self, vals: impl IntoIterator<Item = RVal>) {
        for v in vals {
            self.pool.release_val(v);
        }
    }

    pub(super) fn release_stack_from(&mut self, from: usize) {
        while self.stack.len() > from {
            let v = self.stack.pop().unwrap();
            self.pool.release_val(v);
        }
    }

    pub(super) fn build_closure(
        &mut self,
        proto: &Rc<FnProto>,
        captures: &[RVal],
        base: usize,
        span: Span,
    ) -> Result<RVal, Error> {
        let cap_vals: Rc<[RVal]> = proto
            .captures
            .iter()
            .map(|cap| {
                let v = match cap {
                    Capture::Local(slot) => self.stack[base + *slot as usize],
                    Capture::Upvalue(idx) => captures[*idx as usize],
                };
                self.pool.retain_val(v);
                v
            })
            .collect();
        let id = self.pool.alloc_obj(
            Obj::Closure {
                proto: Rc::clone(proto),
                captures: cap_vals,
            },
            span,
        )?;
        Ok(RVal::Obj(id))
    }

    pub(super) fn run_top_chunk(&mut self, chunk: &Chunk) -> Result<RVal, Error> {
        let base = self.stack.len();
        self.stack
            .resize(base + chunk.local_count as usize, RVal::Void);
        let res = self.exec_frame(
            FrameSource::Top(chunk),
            Rc::from([]),
            base,
            chunk.local_count as usize,
            None,
            self.active_module,
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

    pub(super) fn apply(
        &mut self,
        callee: RVal,
        args: &[RVal],
        span: Span,
        origin: CallOrigin<'_>,
    ) -> Result<RVal, Error> {
        match callee {
            RVal::Obj(id) => {
                let (proto, captures) = match self.pool.get(id) {
                    Obj::Closure { proto, captures } => (Rc::clone(proto), Rc::clone(captures)),
                    _ => {
                        return Err(Error::runtime(
                            "expr.call",
                            span,
                            format!("cannot call value of type {}", callee.type_name(self.pool)),
                        ));
                    }
                };
                if proto.arity as usize != args.len() {
                    return Err(Error::runtime(
                        "expr.call",
                        span,
                        format!("expected {} arguments, got {}", proto.arity, args.len()),
                    ));
                }
                self.enter_depth(span)?;
                let prev_mod = std::mem::replace(&mut self.active_module, proto.module);
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
                    Some(origin),
                    prev_mod,
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
                "expr.call",
                span,
                format!("cannot call value of type {}", other.type_name(self.pool)),
            )),
        }
    }
}

pub(super) enum FrameSource<'a> {
    Top(&'a Chunk),
    Proto(Rc<FnProto>),
}

impl FrameSource<'_> {
    #[inline(always)]
    pub(super) fn chunk(&self) -> &Chunk {
        match self {
            FrameSource::Top(c) => c,
            FrameSource::Proto(p) => &p.chunk,
        }
    }
}
