use std::collections::BTreeMap;

use crate::alloc::{Allocator, HeapAllocator};
use crate::ast::{Expr, ExprKind, Pattern};
use crate::error::{Error, ErrorKind, Limits, Span};
use crate::parser::parse_program;
use crate::pool::{Obj, Pool, RVal};
use crate::value::Value;

const BUILTINS: &[&str] = &[
    "+", "-", "*", "/", "%", "==", "!=", "<", "<=", ">", ">=", "and", "or", "not", "as_f64",
    "as_i64", "sin", "cos", "sqrt", "abs", "floor", "ceil", "list", "len", "is_empty", "get",
    "car", "cdr", "last", "cons", "push", "append", "flatten", "slice", "take", "drop", "reverse",
    "range", "contains", "find", "any", "all", "map", "filter", "flat_map", "fold", "zip",
    "enumerate", "sort", "sort_by", "unique", "list->array", "array->list",
];

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
        for &name in BUILTINS {
            globals.insert(name.to_string(), RVal::Builtin(name));
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
        if let Some(prev_env) = self.module_envs.insert(name.to_string(), BTreeMap::new()) {
            for v in prev_env.into_values() {
                self.pool.release_val(v);
            }
        }
        let mut runner = Runner {
            limits: self.limits,
            steps: 0,
            depth: 0,
            pool: &mut self.pool,
            globals: &mut self.globals,
            modules: &self.modules,
            module_envs: &mut self.module_envs,
            active_module: Some(name.to_string()),
            export_list: None,
        };
        let empty_env = BTreeMap::new();
        for expr in &exprs {
            let v = runner.eval_expr(expr, &empty_env)?;
            runner.pool.release_val(v);
        }
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
        let mut runner = Runner {
            limits: self.limits,
            steps: 0,
            depth: 0,
            pool: &mut self.pool,
            globals: &mut self.globals,
            modules: &self.modules,
            module_envs: &mut self.module_envs,
            active_module: None,
            export_list: None,
        };
        let empty_env = BTreeMap::new();
        let mut last = RVal::Void;
        for expr in &exprs {
            runner.pool.release_val(last);
            last = runner.eval_expr(expr, &empty_env)?;
        }
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
        let mut runner = Runner {
            limits: self.limits,
            steps: 0,
            depth: 0,
            pool: &mut self.pool,
            globals: &mut self.globals,
            modules: &self.modules,
            module_envs: &mut self.module_envs,
            active_module: None,
            export_list: None,
        };
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
    active_module: Option<String>,
    export_list: Option<Vec<String>>,
}

impl Runner<'_, '_> {
    fn tick(&mut self, span: Span) -> Result<(), Error> {
        self.steps += 1;
        if self.steps > self.limits.steps {
            return Err(Error::new(
                ErrorKind::TooManySteps,
                span,
                format!("step limit exceeded ({})", self.limits.steps),
            ));
        }
        Ok(())
    }

    fn enter_depth(&mut self, span: Span) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > self.limits.depth {
            return Err(Error::new(
                ErrorKind::TooDeep,
                span,
                format!("call depth limit exceeded ({})", self.limits.depth),
            ));
        }
        Ok(())
    }

    fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn lookup(&mut self, name: &str, env: &BTreeMap<String, RVal>, span: Span) -> Result<RVal, Error> {
        if let Some(&v) = env.get(name) {
            self.pool.retain_val(v);
            return Ok(v);
        }
        if let Some(mod_name) = &self.active_module
            && let Some(mod_env) = self.module_envs.get(mod_name)
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
            self.module_envs.get_mut(mod_name).unwrap().insert(name, val)
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

    fn retain_env(&mut self, env: &BTreeMap<String, RVal>) -> BTreeMap<String, RVal> {
        for &v in env.values() {
            self.pool.retain_val(v);
        }
        env.clone()
    }

    fn release_env(&mut self, env: BTreeMap<String, RVal>) {
        for v in env.into_values() {
            self.pool.release_val(v);
        }
    }

    fn eval_body(&mut self, body: &[Expr], env: &BTreeMap<String, RVal>) -> Result<RVal, Error> {
        let mut last = RVal::Void;
        for expr in body {
            self.pool.release_val(last);
            last = self.eval_expr(expr, env)?;
        }
        Ok(last)
    }

    fn eval_expr(&mut self, expr: &Expr, env: &BTreeMap<String, RVal>) -> Result<RVal, Error> {
        self.tick(expr.span)?;
        let span = expr.span;

        match &expr.kind {
            ExprKind::Int(n) => Ok(RVal::Int(*n)),
            ExprKind::Float(x) => Ok(RVal::Float(*x)),
            ExprKind::Bool(b) => Ok(RVal::Bool(*b)),
            ExprKind::Char(c) => Ok(RVal::Char(*c)),
            ExprKind::Str(s) => Ok(RVal::Obj(
                self.pool.alloc_obj(Obj::Str(s.clone()), span)?,
            )),
            ExprKind::None => Ok(RVal::None),
            ExprKind::Ident(name) => self.lookup(name, env, span),
            ExprKind::List(items) => {
                let mut evaluated = Vec::with_capacity(items.len());
                for item in items {
                    match self.eval_expr(item, env) {
                        Ok(v) => evaluated.push(v),
                        Err(e) => {
                            self.release_all(evaluated);
                            return Err(e);
                        }
                    }
                }
                let res = self.build_list_from_owned(evaluated, span);
                res
            }
            ExprKind::Tuple(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    match self.eval_expr(item, env) {
                        Ok(v) => out.push(v),
                        Err(e) => {
                            self.release_all(out);
                            return Err(e);
                        }
                    }
                }
                match self.pool.alloc_obj(Obj::Tuple(out.clone()), span) {
                    Ok(id) => Ok(RVal::Obj(id)),
                    Err(e) => {
                        self.release_all(out);
                        Err(e)
                    }
                }
            }
            ExprKind::Struct { name, fields } => {
                let mut map = BTreeMap::new();
                for (k, v_expr) in fields {
                    match self.eval_expr(v_expr, env) {
                        Ok(v) => {
                            if let Some(prev) = map.insert(k.clone(), v) {
                                self.pool.release_val(prev);
                            }
                        }
                        Err(e) => {
                            self.release_env(map);
                            return Err(e);
                        }
                    }
                }
                match self.pool.alloc_obj(
                    Obj::Struct {
                        name: name.clone(),
                        fields: map.clone(),
                    },
                    span,
                ) {
                    Ok(id) => Ok(RVal::Obj(id)),
                    Err(e) => {
                        self.release_env(map);
                        Err(e)
                    }
                }
            }
            ExprKind::Variant {
                type_name,
                variant,
                data,
            } => {
                let val = match data {
                    Some(d) => Some(self.eval_expr(d, env)?),
                    None => None,
                };
                match self.pool.alloc_obj(
                    Obj::Variant {
                        type_name: type_name.clone(),
                        variant: variant.clone(),
                        data: val,
                    },
                    span,
                ) {
                    Ok(id) => Ok(RVal::Obj(id)),
                    Err(e) => {
                        if let Some(v) = val {
                            self.pool.release_val(v);
                        }
                        Err(e)
                    }
                }
            }
            ExprKind::Some(inner) => {
                let val = self.eval_expr(inner, env)?;
                match self.pool.alloc_obj(Obj::Some(val), span) {
                    Ok(id) => Ok(RVal::Obj(id)),
                    Err(e) => {
                        self.pool.release_val(val);
                        Err(e)
                    }
                }
            }
            ExprKind::Ok(inner) => {
                let val = self.eval_expr(inner, env)?;
                match self.pool.alloc_obj(Obj::Ok(val), span) {
                    Ok(id) => Ok(RVal::Obj(id)),
                    Err(e) => {
                        self.pool.release_val(val);
                        Err(e)
                    }
                }
            }
            ExprKind::Err(inner) => {
                let val = self.eval_expr(inner, env)?;
                match self.pool.alloc_obj(Obj::Err(val), span) {
                    Ok(id) => Ok(RVal::Obj(id)),
                    Err(e) => {
                        self.pool.release_val(val);
                        Err(e)
                    }
                }
            }
            ExprKind::Field { target, field } => {
                let val = self.eval_expr(target, env)?;
                let res = match val {
                    RVal::Obj(id) => match self.pool.get(id) {
                        Obj::Struct { name, fields } => match fields.get(field).copied() {
                            Some(fv) => {
                                self.pool.retain_val(fv);
                                Ok(fv)
                            }
                            None => Err(Error::runtime(
                                span,
                                format!("{name} has no field '{field}'"),
                            )),
                        },
                        Obj::Module { name, exports } => match exports.get(field).copied() {
                            Some(fv) => {
                                self.pool.retain_val(fv);
                                Ok(fv)
                            }
                            None => Err(Error::runtime(
                                span,
                                format!("module '{name}' has no export '{field}'"),
                            )),
                        },
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
                res
            }
            ExprKind::Index { target, index } => {
                let list_val = self.eval_expr(target, env)?;
                let idx_val = match self.eval_expr(index, env) {
                    Ok(v) => v,
                    Err(e) => {
                        self.pool.release_val(list_val);
                        return Err(e);
                    }
                };
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
                res
            }
            ExprKind::With { target, fields } => {
                let base = self.eval_expr(target, env)?;
                let (name, mut updated) = match base {
                    RVal::Obj(id) => match self.pool.get(id) {
                        Obj::Struct { name, fields } => (name.clone(), fields.clone()),
                        _ => {
                            let tname = base.type_name(self.pool);
                            self.pool.release_val(base);
                            return Err(Error::runtime(
                                span,
                                format!("'with' requires a struct, got {tname}"),
                            ));
                        }
                    },
                    other => {
                        let tname = other.type_name(self.pool);
                        self.pool.release_val(base);
                        return Err(Error::runtime(
                            span,
                            format!("'with' requires a struct, got {tname}"),
                        ));
                    }
                };
                for &v in updated.values() {
                    self.pool.retain_val(v);
                }
                self.pool.release_val(base);

                for (k, v_expr) in fields {
                    if !updated.contains_key(k) {
                        self.release_env(updated);
                        return Err(Error::runtime(span, format!("{name} has no field '{k}'")));
                    }
                    match self.eval_expr(v_expr, env) {
                        Ok(val) => {
                            if let Some(prev) = updated.insert(k.clone(), val) {
                                self.pool.release_val(prev);
                            }
                        }
                        Err(e) => {
                            self.release_env(updated);
                            return Err(e);
                        }
                    }
                }
                match self.pool.alloc_obj(
                    Obj::Struct {
                        name,
                        fields: updated.clone(),
                    },
                    span,
                ) {
                    Ok(id) => Ok(RVal::Obj(id)),
                    Err(e) => {
                        self.release_env(updated);
                        Err(e)
                    }
                }
            }
            ExprKind::Def { name, value } => {
                let val = self.eval_expr(value, env)?;
                self.define_top(name.clone(), val);
                Ok(RVal::Void)
            }
            ExprKind::Import { name } => {
                let Some(&mod_val) = self.modules.get(name) else {
                    return Err(Error::runtime(span, format!("unknown module '{name}'")));
                };
                self.pool.retain_val(mod_val);
                self.define_top(name.clone(), mod_val);
                Ok(RVal::Void)
            }
            ExprKind::Export { names } => {
                if self.active_module.is_none() {
                    return Err(Error::runtime(
                        span,
                        "'export' can only be used inside a module",
                    ));
                }
                let list = self.export_list.get_or_insert_with(Vec::new);
                list.extend(names.iter().cloned());
                Ok(RVal::Void)
            }
            ExprKind::Defn { name, params, body } => {
                let captured = self.retain_env(env);
                let id = match self.pool.alloc_obj(
                    Obj::Closure {
                        name: Some(name.clone()),
                        module: self.active_module.clone(),
                        params: params.clone(),
                        body: body.clone(),
                        env: captured.clone(),
                    },
                    span,
                ) {
                    Ok(id) => id,
                    Err(e) => {
                        self.release_env(captured);
                        return Err(e);
                    }
                };
                self.define_top(name.clone(), RVal::Obj(id));
                Ok(RVal::Void)
            }
            ExprKind::Fn { params, body } => {
                let captured = self.retain_env(env);
                match self.pool.alloc_obj(
                    Obj::Closure {
                        name: None,
                        module: self.active_module.clone(),
                        params: params.clone(),
                        body: body.clone(),
                        env: captured.clone(),
                    },
                    span,
                ) {
                    Ok(id) => Ok(RVal::Obj(id)),
                    Err(e) => {
                        self.release_env(captured);
                        Err(e)
                    }
                }
            }
            ExprKind::Let { bindings, body } => {
                let mut next = self.retain_env(env);
                for (name, val_expr) in bindings {
                    match self.eval_expr(val_expr, &next) {
                        Ok(val) => {
                            if let Some(prev) = next.insert(name.clone(), val) {
                                self.pool.release_val(prev);
                            }
                        }
                        Err(e) => {
                            self.release_env(next);
                            return Err(e);
                        }
                    }
                }
                let res = self.eval_body(body, &next);
                self.release_env(next);
                res
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                if self.eval_bool(cond, env)? {
                    self.eval_expr(then_branch, env)
                } else {
                    self.eval_expr(else_branch, env)
                }
            }
            ExprKind::When { cond, body } => {
                if self.eval_bool(cond, env)? {
                    let v = self.eval_body(body, env)?;
                    self.pool.release_val(v);
                }
                Ok(RVal::Void)
            }
            ExprKind::Match { scrutinee, arms } => {
                let val = self.eval_expr(scrutinee, env)?;
                for arm in arms {
                    if let Some(extra) = self.match_pattern(&arm.pattern, val) {
                        let mut next = self.retain_env(env);
                        for (k, v) in extra {
                            self.pool.retain_val(v);
                            if let Some(prev) = next.insert(k, v) {
                                self.pool.release_val(prev);
                            }
                        }
                        self.pool.release_val(val);
                        let res = self.eval_body(&arm.body, &next);
                        self.release_env(next);
                        return res;
                    }
                }
                self.pool.release_val(val);
                Err(Error::runtime(span, "non-exhaustive match"))
            }
            ExprKind::Call { callee, args } => {
                if let ExprKind::Ident(op) = &callee.kind {
                    if op == "and" {
                        return self.eval_and(args, env, span);
                    }
                    if op == "or" {
                        return self.eval_or(args, env, span);
                    }
                }
                let callee_val = self.eval_expr(callee, env)?;
                let mut arg_vals = Vec::with_capacity(args.len());
                for arg in args {
                    match self.eval_expr(arg, env) {
                        Ok(v) => arg_vals.push(v),
                        Err(e) => {
                            self.pool.release_val(callee_val);
                            self.release_all(arg_vals);
                            return Err(e);
                        }
                    }
                }
                let res = self.apply(callee_val, &arg_vals, span);
                self.pool.release_val(callee_val);
                self.release_all(arg_vals);
                res
            }
        }
    }

    fn eval_bool(&mut self, expr: &Expr, env: &BTreeMap<String, RVal>) -> Result<bool, Error> {
        let val = self.eval_expr(expr, env)?;
        match val {
            RVal::Bool(b) => Ok(b),
            other => {
                let tname = other.type_name(self.pool);
                self.pool.release_val(val);
                Err(Error::runtime(
                    expr.span,
                    format!("expected bool, got {tname}"),
                ))
            }
        }
    }

    fn eval_and(&mut self, args: &[Expr], env: &BTreeMap<String, RVal>, span: Span) -> Result<RVal, Error> {
        if args.is_empty() {
            return Err(Error::runtime(span, "'and' requires at least 1 argument"));
        }
        for arg in args {
            if !self.eval_bool(arg, env)? {
                return Ok(RVal::Bool(false));
            }
        }
        Ok(RVal::Bool(true))
    }

    fn eval_or(&mut self, args: &[Expr], env: &BTreeMap<String, RVal>, span: Span) -> Result<RVal, Error> {
        if args.is_empty() {
            return Err(Error::runtime(span, "'or' requires at least 1 argument"));
        }
        for arg in args {
            if self.eval_bool(arg, env)? {
                return Ok(RVal::Bool(true));
            }
        }
        Ok(RVal::Bool(false))
    }

    fn apply(&mut self, callee: RVal, args: &[RVal], span: Span) -> Result<RVal, Error> {
        match callee {
            RVal::Obj(id) => {
                let (params, body, captured, mod_owner) = match self.pool.get(id) {
                    Obj::Closure {
                        params,
                        body,
                        env,
                        module,
                        ..
                    } => (params.clone(), body.clone(), env.clone(), module.clone()),
                    _ => {
                        return Err(Error::runtime(
                            span,
                            format!("cannot call value of type {}", callee.type_name(self.pool)),
                        ));
                    }
                };
                if params.len() != args.len() {
                    return Err(Error::runtime(
                        span,
                        format!("expected {} arguments, got {}", params.len(), args.len()),
                    ));
                }
                self.enter_depth(span)?;
                let prev_mod = std::mem::replace(&mut self.active_module, mod_owner);
                let mut next = self.retain_env(&captured);
                for (param, &val) in params.iter().zip(args.iter()) {
                    self.pool.retain_val(val);
                    if let Some(prev) = next.insert(param.clone(), val) {
                        self.pool.release_val(prev);
                    }
                }
                let res = self.eval_body(&body, &next);
                self.release_env(next);
                self.active_module = prev_mod;
                self.leave_depth();
                res
            }
            RVal::Builtin(name) => self.apply_builtin(name, args, span),
            other => Err(Error::runtime(
                span,
                format!("cannot call value of type {}", other.type_name(self.pool)),
            )),
        }
    }

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

    fn apply_builtin(&mut self, name: &str, args: &[RVal], span: Span) -> Result<RVal, Error> {
        match name {
            "+" | "-" | "*" | "/" | "%" => self.arith(name, args, span),
            "==" | "!=" => self.eq_cmp(name == "==", args, span),
            "<" | "<=" | ">" | ">=" => self.ord_cmp(name, args, span),
            "not" => {
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
            "as_f64" => {
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
            "as_i64" => {
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
            "sin" | "cos" | "sqrt" | "abs" | "floor" | "ceil" => self.math(name, args, span),
            "list" => self.build_list_from_borrowed(args, span),
            "len" => {
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
            "is_empty" => {
                let xs = self.expect_one_list("is_empty", args, span)?;
                Ok(RVal::Bool(self.pool.list_len(xs) == 0))
            }
            "get" => {
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
            "car" => {
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
            "cdr" => {
                let xs = self.expect_one_list("cdr", args, span)?;
                let tail = self.list_tail(xs);
                self.pool.retain_val(tail);
                Ok(tail)
            }
            "last" => {
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
            "cons" => {
                let [item, list_v] = args else {
                    return Err(Error::runtime(span, "'cons' takes 2 arguments (item, list)"));
                };
                self.expect_list("cons", *list_v, span)?;
                self.pool.alloc_cons(*item, *list_v, span)
            }
            "push" => {
                let [list_v, item] = args else {
                    return Err(Error::runtime(span, "'push' takes 2 arguments (list, item)"));
                };
                self.expect_list("push", *list_v, span)?;
                let mut items = self.list_to_vec(*list_v);
                items.push(*item);
                self.build_list_from_borrowed(&items, span)
            }
            "append" => {
                let mut items = Vec::new();
                for &arg in args {
                    self.expect_list("append", arg, span)?;
                    items.extend(self.list_to_vec(arg));
                }
                self.build_list_from_borrowed(&items, span)
            }
            "flatten" => {
                let xs = self.expect_one_list("flatten", args, span)?;
                let outer = self.list_to_vec(xs);
                let mut items = Vec::new();
                for sub in outer {
                    self.expect_list("flatten", sub, span)?;
                    items.extend(self.list_to_vec(sub));
                }
                self.build_list_from_borrowed(&items, span)
            }
            "slice" => {
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
            "take" => {
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
            "drop" => {
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
            "reverse" => {
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
            "range" => {
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
                    self.tick(span)?;
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
            "contains" => {
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
            "find" => {
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
            "any" => {
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
            "all" => {
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
            "map" => {
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
            "filter" => {
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
            "flat_map" => {
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
            "fold" => {
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
            "zip" => {
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
            "enumerate" => {
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
            "sort" => {
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
            "sort_by" => {
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
            "unique" => {
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
            "list->array" => {
                let xs = self.expect_one_list("list->array", args, span)?;
                let items = self.list_to_vec(xs);
                for &item in &items {
                    self.pool.retain_val(item);
                }
                match self.pool.alloc_obj(Obj::Array(items.clone()), span) {
                    Ok(id) => Ok(RVal::Obj(id)),
                    Err(e) => {
                        self.release_all(items);
                        Err(e)
                    }
                }
            }
            "array->list" => {
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
            _ => Err(Error::runtime(span, format!("unknown builtin '{name}'"))),
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

    fn arith(&self, op: &str, args: &[RVal], span: Span) -> Result<RVal, Error> {
        if op == "-" && args.len() == 1 {
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
        if (op == "+" || op == "*") && args.len() >= 2 {
            let mut acc = args[0];
            for &rhs in &args[1..] {
                acc = self.arith_bin(op, acc, rhs, span)?;
            }
            return Ok(acc);
        }
        let [a, b] = args else {
            return Err(Error::runtime(span, format!("'{op}' takes 2 arguments")));
        };
        self.arith_bin(op, *a, *b, span)
    }

    fn arith_bin(&self, op: &str, a: RVal, b: RVal, span: Span) -> Result<RVal, Error> {
        match (a, b) {
            (RVal::Int(x), RVal::Int(y)) => {
                let res = match op {
                    "+" => x.checked_add(y),
                    "-" => x.checked_sub(y),
                    "*" => x.checked_mul(y),
                    "/" => {
                        if y == 0 {
                            return Err(Error::runtime(span, "division by zero"));
                        }
                        x.checked_div(y)
                    }
                    "%" => {
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
                "+" => Ok(RVal::Float(x + y)),
                "-" => Ok(RVal::Float(x - y)),
                "*" => Ok(RVal::Float(x * y)),
                "/" => Ok(RVal::Float(x / y)),
                "%" => Ok(RVal::Float(x % y)),
                _ => unreachable!(),
            },
            _ => Err(Error::runtime(
                span,
                format!(
                    "cannot apply '{op}' to {} and {} (no implicit conversion)",
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

    fn ord_cmp(&self, op: &str, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [a, b] = args else {
            return Err(Error::runtime(span, format!("'{op}' takes 2 arguments")));
        };
        let res = match (*a, *b) {
            (RVal::Int(x), RVal::Int(y)) => match op {
                "<" => x < y,
                "<=" => x <= y,
                ">" => x > y,
                ">=" => x >= y,
                _ => unreachable!(),
            },
            (RVal::Float(x), RVal::Float(y)) => match op {
                "<" => x < y,
                "<=" => x <= y,
                ">" => x > y,
                ">=" => x >= y,
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

    fn math(&self, name: &str, args: &[RVal], span: Span) -> Result<RVal, Error> {
        let [v] = args else {
            return Err(Error::runtime(span, format!("'{name}' takes 1 argument")));
        };
        if name == "abs" {
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
                format!("'{name}' expects f64, got {}", v.type_name(self.pool)),
            ));
        };
        let res = match name {
            "sin" => x.sin(),
            "cos" => x.cos(),
            "sqrt" => x.sqrt(),
            "floor" => x.floor(),
            "ceil" => x.ceil(),
            _ => unreachable!(),
        };
        Ok(RVal::Float(res))
    }

    fn match_pattern(&self, pat: &Pattern, val: RVal) -> Option<Vec<(String, RVal)>> {
        match (pat, val) {
            (Pattern::Wildcard, _) => Some(Vec::new()),
            (Pattern::Var(name), v) => Some(vec![(name.clone(), v)]),
            (Pattern::Int(a), RVal::Int(b)) if *a == b => Some(Vec::new()),
            (Pattern::Bool(a), RVal::Bool(b)) if *a == b => Some(Vec::new()),
            (Pattern::Char(a), RVal::Char(b)) if *a == b => Some(Vec::new()),
            (Pattern::Str(a), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Str(s) if a == s => Some(Vec::new()),
                _ => None,
            },
            (Pattern::None, RVal::None) => Some(Vec::new()),
            (Pattern::Some(bind), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Some(inner) => Some(bind.iter().map(|n| (n.clone(), *inner)).collect()),
                _ => None,
            },
            (Pattern::Ok(bind), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Ok(inner) => Some(bind.iter().map(|n| (n.clone(), *inner)).collect()),
                _ => None,
            },
            (Pattern::Err(bind), RVal::Obj(id)) => match self.pool.get(id) {
                Obj::Err(inner) => Some(bind.iter().map(|n| (n.clone(), *inner)).collect()),
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
                    (Some(name), Some(d)) => Some(vec![(name.clone(), *d)]),
                    (None, _) => Some(Vec::new()),
                    (Some(_), None) => None,
                },
                _ => None,
            },
            _ => None,
        }
    }
}
