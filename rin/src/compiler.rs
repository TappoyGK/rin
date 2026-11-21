//! AST をスロット解決付きのバイトコードチャンクへコンパイルする。

use std::rc::Rc;

use crate::ast::{Expr, ExprKind, Pattern};
use crate::bytecode::{Capture, Chunk, FnProto, Op, StructDesc, SymTable};
use crate::error::{Error, Span};

pub(crate) fn compile_program(
    exprs: &[Expr],
    module: Option<u32>,
    syms: &mut SymTable,
) -> Result<Chunk, Error> {
    let mut compiler = Compiler {
        scopes: vec![Scope::new(module)],
        module,
        syms,
    };
    if exprs.is_empty() {
        compiler.chunk().emit(Op::Void, Span::default());
    } else {
        for (i, expr) in exprs.iter().enumerate() {
            compiler.compile_expr(expr, false)?;
            if i + 1 < exprs.len() {
                compiler.emit(Op::Pop, expr.span);
            }
        }
        compiler.flush_ticks();
    }
    let mut finished = compiler.scopes.pop().unwrap();
    finished.chunk.local_count = finished.max_slots;
    Ok(finished.chunk)
}

struct Scope {
    module: Option<u32>,
    locals: Vec<(String, u32)>,
    next_slot: u32,
    max_slots: u32,
    captures: Vec<(String, Capture)>,
    pending_tick_spans: Vec<Span>,
    chunk: Chunk,
}

impl Scope {
    fn new(module: Option<u32>) -> Self {
        Self {
            module,
            locals: Vec::new(),
            next_slot: 0,
            max_slots: 0,
            captures: Vec::new(),
            pending_tick_spans: Vec::new(),
            chunk: Chunk::default(),
        }
    }

    fn alloc_slot(&mut self, name: String, span: Span) -> Result<u32, Error> {
        let slot = self.next_slot;
        self.next_slot = self
            .next_slot
            .checked_add(1)
            .ok_or_else(|| Error::parse(span, "too many local variables"))?;
        if self.next_slot > self.max_slots {
            self.max_slots = self.next_slot;
        }
        self.locals.push((name, slot));
        Ok(slot)
    }

    fn find_local(&self, name: &str) -> Option<u32> {
        self.locals
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, slot)| *slot)
    }

    fn find_or_add_capture(&mut self, name: &str, cap: Capture, span: Span) -> Result<u32, Error> {
        if let Some(pos) = self.captures.iter().position(|(n, _)| n == name) {
            return Ok(pos as u32);
        }
        let idx = u32::try_from(self.captures.len())
            .map_err(|_| Error::parse(span, "too many captured variables"))?;
        self.captures.push((name.to_string(), cap));
        Ok(idx)
    }
}

struct Compiler<'a> {
    scopes: Vec<Scope>,
    module: Option<u32>,
    syms: &'a mut SymTable,
}

impl Compiler<'_> {
    fn chunk(&mut self) -> &mut Chunk {
        &mut self.scopes.last_mut().unwrap().chunk
    }

    fn flush_ticks(&mut self) {
        let top = self.scopes.last_mut().unwrap();
        if !top.pending_tick_spans.is_empty() {
            let start = top.chunk.tick_spans.len() as u32;
            let count = top.pending_tick_spans.len() as u32;
            let first_span = top.pending_tick_spans[0];
            top.chunk.tick_spans.append(&mut top.pending_tick_spans);
            top.chunk.emit(Op::Tick(start, count), first_span);
        }
    }

    fn emit(&mut self, op: Op, span: Span) -> usize {
        self.flush_ticks();
        self.chunk().emit(op, span)
    }

    fn resolve_var(&mut self, name: &str, span: Span) -> Result<Op, Error> {
        let top = self.scopes.len() - 1;
        if let Some(slot) = self.scopes[top].find_local(name) {
            return Ok(Op::GetLocal(slot));
        }
        if top > 0
            && let Some(cap_idx) = self.resolve_capture(top, name, span)?
        {
            return Ok(Op::GetCapture(cap_idx));
        }
        let sym_id = self.syms.intern(name);
        Ok(Op::GetGlobal(sym_id))
    }

    fn resolve_capture(
        &mut self,
        scope_idx: usize,
        name: &str,
        span: Span,
    ) -> Result<Option<u32>, Error> {
        if scope_idx == 0 {
            return Ok(None);
        }
        if let Some(pos) = self.scopes[scope_idx]
            .captures
            .iter()
            .position(|(n, _)| n == name)
        {
            return Ok(Some(pos as u32));
        }
        let parent = scope_idx - 1;
        if let Some(slot) = self.scopes[parent].find_local(name) {
            return Ok(Some(self.scopes[scope_idx].find_or_add_capture(
                name,
                Capture::Local(slot),
                span,
            )?));
        }
        if parent > 0
            && let Some(up_idx) = self.resolve_capture(parent, name, span)?
        {
            return Ok(Some(self.scopes[scope_idx].find_or_add_capture(
                name,
                Capture::Upvalue(up_idx),
                span,
            )?));
        }
        Ok(None)
    }

    fn compile_body(&mut self, body: &[Expr], is_tail: bool) -> Result<(), Error> {
        for (i, expr) in body.iter().enumerate() {
            let last = i + 1 == body.len();
            self.compile_expr(expr, is_tail && last)?;
            if !last {
                self.emit(Op::Pop, expr.span);
            }
        }
        if !body.is_empty() {
            self.flush_ticks();
        }
        Ok(())
    }

    fn compile_fn_proto(
        &mut self,
        name: Option<String>,
        params: &[String],
        body: &[Expr],
        span: Span,
    ) -> Result<Rc<FnProto>, Error> {
        let arity =
            u32::try_from(params.len()).map_err(|_| Error::parse(span, "too many parameters"))?;
        let mut inner = Scope::new(self.module);
        for p in params {
            inner.alloc_slot(p.clone(), span)?;
        }
        self.scopes.push(inner);
        self.compile_body(body, true)?;
        let finished = self.scopes.pop().unwrap();
        let captures: Vec<Capture> = finished.captures.into_iter().map(|(_, c)| c).collect();
        Ok(Rc::new(FnProto {
            name,
            module: finished.module,
            arity,
            local_count: finished.max_slots,
            captures,
            chunk: finished.chunk,
        }))
    }

    fn patch_jump(&mut self, op_idx: usize, target: usize) {
        let code = &mut self.chunk().code;
        match &mut code[op_idx] {
            Op::Jump(t) | Op::JumpIfFalse(t) | Op::JumpIfFalsePeek(t) | Op::JumpIfTruePeek(t) => {
                *t = target
            }
            Op::TryMatch { fail_ip, .. } => *fail_ip = target,
            _ => unreachable!(),
        }
    }

    fn compile_expr(&mut self, expr: &Expr, is_tail: bool) -> Result<(), Error> {
        let span = expr.span;
        self.scopes
            .last_mut()
            .unwrap()
            .pending_tick_spans
            .push(span);

        match &expr.kind {
            ExprKind::Int(n) => {
                self.chunk().emit(Op::Int(*n), span);
            }
            ExprKind::Float(x) => {
                let idx = self.chunk().add_float(*x);
                self.chunk().emit(Op::Float(idx), span);
            }
            ExprKind::Bool(b) => {
                self.chunk().emit(Op::Bool(*b), span);
            }
            ExprKind::Char(c) => {
                self.chunk().emit(Op::Char(*c), span);
            }
            ExprKind::Str(s) => {
                let idx = self.chunk().add_string(s);
                self.emit(Op::Str(idx), span);
            }
            ExprKind::None => {
                self.chunk().emit(Op::None, span);
            }
            ExprKind::Ident(name) => {
                let op = self.resolve_var(name, span)?;
                match op {
                    Op::GetGlobal(_) => {
                        self.emit(op, span);
                    }
                    _ => {
                        self.chunk().emit(op, span);
                    }
                }
            }
            ExprKind::List(items) => {
                let count = u32::try_from(items.len())
                    .map_err(|_| Error::parse(span, "list literal too large"))?;
                if count == 0 {
                    self.emit(Op::EmptyList, span);
                } else {
                    for item in items {
                        self.compile_expr(item, false)?;
                    }
                    self.emit(Op::MakeList(count), span);
                }
            }
            ExprKind::Tuple(items) => {
                let count = u32::try_from(items.len())
                    .map_err(|_| Error::parse(span, "tuple literal too large"))?;
                for item in items {
                    self.compile_expr(item, false)?;
                }
                self.emit(Op::MakeTuple(count), span);
            }
            ExprKind::Struct { name, fields } => {
                let mut field_names = Vec::with_capacity(fields.len());
                for (fname, fexpr) in fields {
                    self.compile_expr(fexpr, false)?;
                    field_names.push(Rc::from(fname.as_str()));
                }
                let s_idx = self.chunk().add_struct(StructDesc {
                    name: Rc::from(name.as_str()),
                    fields: field_names,
                });
                self.emit(Op::MakeStruct(s_idx), span);
            }
            ExprKind::With { target, fields } => {
                self.compile_expr(target, false)?;
                self.emit(Op::WithCheckStruct, span);
                let mut field_names = Vec::with_capacity(fields.len());
                for (idx, (fname, fexpr)) in fields.iter().enumerate() {
                    let f_idx = self.chunk().add_string(fname);
                    self.emit(Op::WithCheckField(f_idx, idx as u32), span);
                    self.compile_expr(fexpr, false)?;
                    field_names.push(Rc::from(fname.as_str()));
                }
                let s_idx = self.chunk().add_struct(StructDesc {
                    name: Rc::from(""),
                    fields: field_names,
                });
                self.emit(Op::WithFinish(s_idx), span);
            }
            ExprKind::Variant {
                type_name,
                variant,
                data,
            } => {
                let has_data = data.is_some();
                if let Some(d) = data {
                    self.compile_expr(d, false)?;
                }
                let t_idx = self.chunk().add_string(type_name);
                let v_idx = self.chunk().add_string(variant);
                self.emit(
                    Op::MakeVariant {
                        type_name: t_idx,
                        variant: v_idx,
                        has_data,
                    },
                    span,
                );
            }
            ExprKind::Some(inner) => {
                self.compile_expr(inner, false)?;
                self.emit(Op::MakeSome, span);
            }
            ExprKind::Ok(inner) => {
                self.compile_expr(inner, false)?;
                self.emit(Op::MakeOk, span);
            }
            ExprKind::Err(inner) => {
                self.compile_expr(inner, false)?;
                self.emit(Op::MakeErr, span);
            }
            ExprKind::Field { target, field } => {
                self.compile_expr(target, false)?;
                let f_idx = self.chunk().add_string(field);
                self.emit(Op::GetField(f_idx), span);
            }
            ExprKind::Index { target, index } => {
                self.compile_expr(target, false)?;
                self.compile_expr(index, false)?;
                self.emit(Op::GetIndex, span);
            }
            ExprKind::Def { name, value } => {
                self.compile_expr(value, false)?;
                let n_idx = self.syms.intern(name);
                self.emit(Op::DefGlobal(n_idx), span);
            }
            ExprKind::Defn { name, params, body } => {
                let proto = self.compile_fn_proto(Some(name.clone()), params, body, span)?;
                let p_idx = self.chunk().add_proto(proto);
                let n_idx = self.syms.intern(name);
                self.emit(Op::DefFn(n_idx, p_idx), span);
            }
            ExprKind::Fn { params, body } => {
                let proto = self.compile_fn_proto(None, params, body, span)?;
                let p_idx = self.chunk().add_proto(proto);
                self.emit(Op::MakeClosure(p_idx), span);
            }
            ExprKind::Let { bindings, body } => {
                let saved_locals_len = self.scopes.last().unwrap().locals.len();
                let saved_next_slot = self.scopes.last().unwrap().next_slot;
                for (name, val_expr) in bindings {
                    self.compile_expr(val_expr, false)?;
                    let slot = self
                        .scopes
                        .last_mut()
                        .unwrap()
                        .alloc_slot(name.clone(), val_expr.span)?;
                    self.chunk().emit(Op::SetLocal(slot), val_expr.span);
                }
                self.compile_body(body, is_tail)?;
                let scope = self.scopes.last_mut().unwrap();
                scope.locals.truncate(saved_locals_len);
                scope.next_slot = saved_next_slot;
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.compile_expr(cond, false)?;
                let j_else = self.emit(Op::JumpIfFalse(0), cond.span);
                self.compile_expr(then_branch, is_tail)?;
                let j_end = self.emit(Op::Jump(0), span);
                let else_ip = self.chunk().code.len();
                self.patch_jump(j_else, else_ip);
                self.compile_expr(else_branch, is_tail)?;
                self.flush_ticks();
                let end_ip = self.chunk().code.len();
                self.patch_jump(j_end, end_ip);
            }
            ExprKind::When { cond, body } => {
                self.compile_expr(cond, false)?;
                let j_end = self.emit(Op::JumpIfFalse(0), cond.span);
                self.compile_body(body, false)?;
                self.emit(Op::Pop, span);
                let end_ip = self.chunk().code.len();
                self.patch_jump(j_end, end_ip);
                self.chunk().emit(Op::Void, span);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.compile_expr(scrutinee, false)?;
                let mut end_jumps = Vec::with_capacity(arms.len());
                for arm in arms {
                    let pat_idx = self.chunk().add_pattern(arm.pattern.clone());
                    let try_ip = self.emit(
                        Op::TryMatch {
                            pat_idx,
                            fail_ip: 0,
                        },
                        arm.span,
                    );
                    let saved_locals_len = self.scopes.last().unwrap().locals.len();
                    let saved_next_slot = self.scopes.last().unwrap().next_slot;
                    if let Some(bind_name) = pattern_binding(&arm.pattern) {
                        let slot = self
                            .scopes
                            .last_mut()
                            .unwrap()
                            .alloc_slot(bind_name, arm.span)?;
                        self.chunk().emit(Op::SetLocal(slot), arm.span);
                    }
                    self.chunk().emit(Op::Pop, arm.span);
                    self.compile_body(&arm.body, is_tail)?;
                    let scope = self.scopes.last_mut().unwrap();
                    scope.locals.truncate(saved_locals_len);
                    scope.next_slot = saved_next_slot;
                    end_jumps.push(self.emit(Op::Jump(0), arm.span));
                    let fail_ip = self.chunk().code.len();
                    self.patch_jump(try_ip, fail_ip);
                }
                self.chunk().emit(Op::Pop, span);
                self.emit(Op::MatchFail, span);
                let end_ip = self.chunk().code.len();
                for j in end_jumps {
                    self.patch_jump(j, end_ip);
                }
            }
            ExprKind::Call { callee, args } => {
                if let ExprKind::Ident(op) = &callee.kind {
                    if op == "and" {
                        return self.compile_short_circuit(true, args, span);
                    }
                    if op == "or" {
                        return self.compile_short_circuit(false, args, span);
                    }
                }
                let argc = u32::try_from(args.len())
                    .map_err(|_| Error::parse(span, "too many arguments"))?;
                self.compile_expr(callee, false)?;
                for arg in args {
                    self.compile_expr(arg, false)?;
                }
                let call_op = if is_tail {
                    Op::TailCall(argc)
                } else {
                    Op::Call(argc)
                };
                self.emit(call_op, span);
            }
            ExprKind::Import { name } => {
                let idx = self.syms.intern(name);
                self.emit(Op::Import(idx), span);
            }
            ExprKind::Export { names } => {
                let idx = self.chunk().add_exports(names.clone());
                self.emit(Op::Export(idx), span);
            }
        }
        Ok(())
    }

    fn compile_short_circuit(
        &mut self,
        is_and: bool,
        args: &[Expr],
        span: Span,
    ) -> Result<(), Error> {
        if args.is_empty() {
            let op = if is_and { Op::And } else { Op::Or };
            self.emit(op, span);
            return Ok(());
        }
        let mut jumps = Vec::with_capacity(args.len());
        for arg in args {
            self.compile_expr(arg, false)?;
            let j = if is_and {
                self.emit(Op::JumpIfFalsePeek(0), arg.span)
            } else {
                self.emit(Op::JumpIfTruePeek(0), arg.span)
            };
            jumps.push(j);
        }
        self.chunk().emit(Op::Bool(is_and), span);
        let end_ip = self.chunk().code.len();
        for j in jumps {
            self.patch_jump(j, end_ip);
        }
        Ok(())
    }
}

fn pattern_binding(pat: &Pattern) -> Option<String> {
    match pat {
        Pattern::Var(name) => Some(name.clone()),
        Pattern::Some(Some(name))
        | Pattern::Ok(Some(name))
        | Pattern::Err(Some(name))
        | Pattern::Variant {
            binding: Some(name),
            ..
        } => Some(name.clone()),
        _ => None,
    }
}
