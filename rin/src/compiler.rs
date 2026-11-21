use std::rc::Rc;

use crate::ast::{Expr, ExprKind, Pattern};
use crate::bytecode::{Capture, Chunk, FnProto, Op, StructDesc};
use crate::error::{Error, Span};

pub(crate) fn compile_program(exprs: &[Expr], module: Option<String>) -> Result<Chunk, Error> {
    let mut compiler = Compiler {
        scopes: vec![Scope::new(module.clone())],
        module,
    };
    if exprs.is_empty() {
        compiler.chunk().emit(Op::Void, Span::default());
    } else {
        for (i, expr) in exprs.iter().enumerate() {
            compiler.compile_expr(expr)?;
            if i + 1 < exprs.len() {
                compiler.chunk().emit(Op::Pop, expr.span);
            }
        }
    }
    let mut finished = compiler.scopes.pop().unwrap();
    finished.chunk.local_count = finished.max_slots;
    Ok(finished.chunk)
}

struct Scope {
    module: Option<String>,
    locals: Vec<(String, u16)>,
    next_slot: u16,
    max_slots: u16,
    captures: Vec<(String, Capture)>,
    chunk: Chunk,
}

impl Scope {
    fn new(module: Option<String>) -> Self {
        Self {
            module,
            locals: Vec::new(),
            next_slot: 0,
            max_slots: 0,
            captures: Vec::new(),
            chunk: Chunk::default(),
        }
    }

    fn alloc_slot(&mut self, name: String) -> u16 {
        let slot = self.next_slot;
        self.next_slot += 1;
        if self.next_slot > self.max_slots {
            self.max_slots = self.next_slot;
        }
        self.locals.push((name, slot));
        slot
    }

    fn find_local(&self, name: &str) -> Option<u16> {
        self.locals
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, slot)| *slot)
    }

    fn find_or_add_capture(&mut self, name: &str, cap: Capture) -> u16 {
        if let Some(pos) = self.captures.iter().position(|(n, _)| n == name) {
            return pos as u16;
        }
        let idx = self.captures.len() as u16;
        self.captures.push((name.to_string(), cap));
        idx
    }
}

struct Compiler {
    scopes: Vec<Scope>,
    module: Option<String>,
}

impl Compiler {
    fn chunk(&mut self) -> &mut Chunk {
        &mut self.scopes.last_mut().unwrap().chunk
    }

    fn resolve_var(&mut self, name: &str) -> Op {
        let top = self.scopes.len() - 1;
        if let Some(slot) = self.scopes[top].find_local(name) {
            return Op::GetLocal(slot);
        }
        if top > 0 {
            if let Some(cap_idx) = self.resolve_capture(top, name) {
                return Op::GetCapture(cap_idx);
            }
        }
        let str_idx = self.chunk().add_string(name.to_string());
        Op::GetGlobal(str_idx)
    }

    fn resolve_capture(&mut self, scope_idx: usize, name: &str) -> Option<u16> {
        if scope_idx == 0 {
            return None;
        }
        // Check if already captured in `scope_idx`
        if let Some(pos) = self.scopes[scope_idx]
            .captures
            .iter()
            .position(|(n, _)| n == name)
        {
            return Some(pos as u16);
        }
        let parent = scope_idx - 1;
        if let Some(slot) = self.scopes[parent].find_local(name) {
            return Some(self.scopes[scope_idx].find_or_add_capture(name, Capture::Local(slot)));
        }
        if parent > 0 {
            if let Some(up_idx) = self.resolve_capture(parent, name) {
                return Some(
                    self.scopes[scope_idx].find_or_add_capture(name, Capture::Upvalue(up_idx)),
                );
            }
        }
        None
    }

    fn compile_body(&mut self, body: &[Expr]) -> Result<(), Error> {
        for (i, expr) in body.iter().enumerate() {
            self.compile_expr(expr)?;
            if i + 1 < body.len() {
                self.chunk().emit(Op::Pop, expr.span);
            }
        }
        Ok(())
    }

    fn compile_fn_proto(
        &mut self,
        name: Option<String>,
        params: &[String],
        body: &[Expr],
    ) -> Result<Rc<FnProto>, Error> {
        let mut inner = Scope::new(self.module.clone());
        for p in params {
            inner.alloc_slot(p.clone());
        }
        self.scopes.push(inner);
        self.compile_body(body)?;
        let finished = self.scopes.pop().unwrap();
        let captures = finished.captures.into_iter().map(|(_, c)| c).collect();
        Ok(Rc::new(FnProto {
            name,
            module: finished.module,
            arity: params.len() as u16,
            local_count: finished.max_slots,
            captures,
            chunk: finished.chunk,
        }))
    }

    fn patch_jump(&mut self, op_idx: usize, target: usize) {
        let code = &mut self.chunk().code;
        match &mut code[op_idx] {
            Op::Jump(t)
            | Op::JumpIfFalse(t)
            | Op::JumpIfFalsePeek(t)
            | Op::JumpIfTruePeek(t) => *t = target,
            Op::TryMatch { fail_ip, .. } => *fail_ip = target,
            _ => unreachable!(),
        }
    }

    fn compile_expr(&mut self, expr: &Expr) -> Result<(), Error> {
        let span = expr.span;
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
                let idx = self.chunk().add_string(s.clone());
                self.chunk().emit(Op::Str(idx), span);
            }
            ExprKind::None => {
                self.chunk().emit(Op::None, span);
            }
            ExprKind::Ident(name) => {
                let op = self.resolve_var(name);
                self.chunk().emit(op, span);
            }
            ExprKind::List(items) => {
                if items.is_empty() {
                    self.chunk().emit(Op::EmptyList, span);
                } else {
                    for item in items {
                        self.compile_expr(item)?;
                    }
                    self.chunk().emit(Op::MakeList(items.len() as u16), span);
                }
            }
            ExprKind::Tuple(items) => {
                for item in items {
                    self.compile_expr(item)?;
                }
                self.chunk().emit(Op::MakeTuple(items.len() as u16), span);
            }
            ExprKind::Struct { name, fields } => {
                let mut field_names = Vec::with_capacity(fields.len());
                for (fname, fexpr) in fields {
                    self.compile_expr(fexpr)?;
                    field_names.push(fname.clone());
                }
                let s_idx = self.chunk().add_struct(StructDesc {
                    name: name.clone(),
                    fields: field_names,
                });
                self.chunk().emit(Op::MakeStruct(s_idx), span);
            }
            ExprKind::With { target, fields } => {
                self.compile_expr(target)?;
                let mut field_names = Vec::with_capacity(fields.len());
                for (fname, fexpr) in fields {
                    self.compile_expr(fexpr)?;
                    field_names.push(fname.clone());
                }
                let s_idx = self.chunk().add_struct(StructDesc {
                    name: String::new(),
                    fields: field_names,
                });
                self.chunk().emit(Op::WithStruct(s_idx), span);
            }
            ExprKind::Variant {
                type_name,
                variant,
                data,
            } => {
                let has_data = data.is_some();
                if let Some(d) = data {
                    self.compile_expr(d)?;
                }
                let t_idx = self.chunk().add_string(type_name.clone());
                let v_idx = self.chunk().add_string(variant.clone());
                self.chunk().emit(
                    Op::MakeVariant {
                        type_name: t_idx,
                        variant: v_idx,
                        has_data,
                    },
                    span,
                );
            }
            ExprKind::Some(inner) => {
                self.compile_expr(inner)?;
                self.chunk().emit(Op::MakeSome, span);
            }
            ExprKind::Ok(inner) => {
                self.compile_expr(inner)?;
                self.chunk().emit(Op::MakeOk, span);
            }
            ExprKind::Err(inner) => {
                self.compile_expr(inner)?;
                self.chunk().emit(Op::MakeErr, span);
            }
            ExprKind::Field { target, field } => {
                self.compile_expr(target)?;
                let f_idx = self.chunk().add_string(field.clone());
                self.chunk().emit(Op::GetField(f_idx), span);
            }
            ExprKind::Index { target, index } => {
                self.compile_expr(target)?;
                self.compile_expr(index)?;
                self.chunk().emit(Op::GetIndex, span);
            }
            ExprKind::Def { name, value } => {
                self.compile_expr(value)?;
                let n_idx = self.chunk().add_string(name.clone());
                self.chunk().emit(Op::DefGlobal(n_idx), span);
            }
            ExprKind::Defn { name, params, body } => {
                let proto = self.compile_fn_proto(Some(name.clone()), params, body)?;
                let p_idx = self.chunk().add_proto(proto);
                let n_idx = self.chunk().add_string(name.clone());
                self.chunk().emit(Op::DefFn(n_idx, p_idx), span);
            }
            ExprKind::Fn { params, body } => {
                let proto = self.compile_fn_proto(None, params, body)?;
                let p_idx = self.chunk().add_proto(proto);
                self.chunk().emit(Op::MakeClosure(p_idx), span);
            }
            ExprKind::Let { bindings, body } => {
                let saved_locals_len = self.scopes.last().unwrap().locals.len();
                let saved_next_slot = self.scopes.last().unwrap().next_slot;
                for (name, val_expr) in bindings {
                    self.compile_expr(val_expr)?;
                    let slot = self.scopes.last_mut().unwrap().alloc_slot(name.clone());
                    self.chunk().emit(Op::SetLocal(slot), val_expr.span);
                }
                self.compile_body(body)?;
                let scope = self.scopes.last_mut().unwrap();
                scope.locals.truncate(saved_locals_len);
                scope.next_slot = saved_next_slot;
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.compile_expr(cond)?;
                let j_else = self.chunk().emit(Op::JumpIfFalse(0), cond.span);
                self.compile_expr(then_branch)?;
                let j_end = self.chunk().emit(Op::Jump(0), span);
                let else_ip = self.chunk().code.len();
                self.patch_jump(j_else, else_ip);
                self.compile_expr(else_branch)?;
                let end_ip = self.chunk().code.len();
                self.patch_jump(j_end, end_ip);
            }
            ExprKind::When { cond, body } => {
                self.compile_expr(cond)?;
                let j_end = self.chunk().emit(Op::JumpIfFalse(0), cond.span);
                self.compile_body(body)?;
                self.chunk().emit(Op::Pop, span);
                let end_ip = self.chunk().code.len();
                self.patch_jump(j_end, end_ip);
                self.chunk().emit(Op::Void, span);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.compile_expr(scrutinee)?;
                let mut end_jumps = Vec::with_capacity(arms.len());
                for arm in arms {
                    let pat_idx = self.chunk().add_pattern(arm.pattern.clone());
                    let try_ip = self.chunk().emit(
                        Op::TryMatch {
                            pat_idx,
                            fail_ip: 0,
                        },
                        arm.span,
                    );
                    let saved_locals_len = self.scopes.last().unwrap().locals.len();
                    let saved_next_slot = self.scopes.last().unwrap().next_slot;
                    if let Some(bind_name) = pattern_binding(&arm.pattern) {
                        let slot = self.scopes.last_mut().unwrap().alloc_slot(bind_name);
                        self.chunk().emit(Op::SetLocal(slot), arm.span);
                    }
                    self.chunk().emit(Op::Pop, arm.span);
                    self.compile_body(&arm.body)?;
                    let scope = self.scopes.last_mut().unwrap();
                    scope.locals.truncate(saved_locals_len);
                    scope.next_slot = saved_next_slot;
                    end_jumps.push(self.chunk().emit(Op::Jump(0), arm.span));
                    let fail_ip = self.chunk().code.len();
                    self.patch_jump(try_ip, fail_ip);
                }
                self.chunk().emit(Op::Pop, span);
                self.chunk().emit(Op::MatchFail, span);
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
                self.compile_expr(callee)?;
                for arg in args {
                    self.compile_expr(arg)?;
                }
                self.chunk().emit(Op::Call(args.len() as u16), span);
            }
            ExprKind::Import { name } => {
                let idx = self.chunk().add_string(name.clone());
                self.chunk().emit(Op::Import(idx), span);
            }
            ExprKind::Export { names } => {
                let idx = self.chunk().add_exports(names.clone());
                self.chunk().emit(Op::Export(idx), span);
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
            self.chunk().emit(op, span);
            return Ok(());
        }
        let mut jumps = Vec::with_capacity(args.len());
        for arg in args {
            self.compile_expr(arg)?;
            let j = if is_and {
                self.chunk().emit(Op::JumpIfFalsePeek(0), arg.span)
            } else {
                self.chunk().emit(Op::JumpIfTruePeek(0), arg.span)
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
