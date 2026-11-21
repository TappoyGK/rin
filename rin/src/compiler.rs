//! AST をスロット解決付きのバイトコードチャンクへコンパイルする。
//!
//! 深い入れ子でもネイティブスタックを消費しないよう、明示的タスクスタックでコンパイルする。

use std::rc::Rc;

use crate::ast::{Expr, ExprKind, MatchArm, Pattern};
use crate::bytecode::{Capture, Chunk, FnProto, Op, StructDesc, SymTable};
use crate::error::{Error, Span};

/// AST 式列をトップレベルのバイトコードチャンクへコンパイルする。
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
        let mut steps = Vec::new();
        steps.push(Step::FlushTicks);
        compiler.push_body(&mut steps, exprs, false);
        compiler.run_steps(&mut steps)?;
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

enum Step<'a> {
    Expr {
        expr: &'a Expr,
        is_tail: bool,
    },
    EmitFlushed {
        op: Op,
        span: Span,
    },
    EmitRaw {
        op: Op,
        span: Span,
    },
    FlushTicks,
    BindLocal {
        name: String,
        span: Span,
    },
    RestoreLocals {
        saved_locals_len: usize,
        saved_next_slot: u32,
    },
    FinishStruct {
        name: &'a str,
        field_names: Vec<Rc<str>>,
        span: Span,
    },
    FinishWith {
        field_names: Vec<Rc<str>>,
        span: Span,
    },
    FinishFn {
        name: Option<String>,
        arity: u32,
        is_defn: bool,
        span: Span,
    },
    IfAfterCond {
        then_branch: &'a Expr,
        else_branch: &'a Expr,
        is_tail: bool,
        cond_span: Span,
        span: Span,
    },
    IfAfterThen {
        j_else: usize,
        else_branch: &'a Expr,
        is_tail: bool,
        span: Span,
    },
    PatchJump {
        op_idx: usize,
    },
    WhenAfterCond {
        body: &'a [Expr],
        cond_span: Span,
        span: Span,
    },
    WhenAfterBody {
        j_end: usize,
        span: Span,
    },
    MatchStartArms {
        arms: &'a [MatchArm],
        idx: usize,
        end_jumps: Vec<usize>,
        is_tail: bool,
        span: Span,
    },
    MatchAfterArm {
        arms: &'a [MatchArm],
        idx: usize,
        try_ip: usize,
        saved_locals_len: usize,
        saved_next_slot: u32,
        end_jumps: Vec<usize>,
        is_tail: bool,
        span: Span,
    },
    ShortCircuitAfterArg {
        is_and: bool,
        args: &'a [Expr],
        idx: usize,
        jumps: Vec<usize>,
        span: Span,
    },
}

struct Compiler<'a> {
    scopes: Vec<Scope>,
    module: Option<u32>,
    syms: &'a mut SymTable,
}

impl<'a> Compiler<'a> {
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
        let id = self.syms.intern(name);
        Ok(Op::GetGlobal(id))
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
        let parent = scope_idx - 1;
        if let Some(slot) = self.scopes[parent].find_local(name) {
            let cap_idx =
                self.scopes[scope_idx].find_or_add_capture(name, Capture::Local(slot), span)?;
            return Ok(Some(cap_idx));
        }
        if parent > 0
            && let Some(parent_cap) = self.resolve_capture(parent, name, span)?
        {
            let cap_idx = self.scopes[scope_idx].find_or_add_capture(
                name,
                Capture::Upvalue(parent_cap),
                span,
            )?;
            return Ok(Some(cap_idx));
        }
        Ok(None)
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

    fn push_body(&self, steps: &mut Vec<Step<'a>>, body: &'a [Expr], is_tail: bool) {
        if body.is_empty() {
            steps.push(Step::EmitRaw {
                op: Op::Void,
                span: Span::default(),
            });
            return;
        }
        steps.push(Step::FlushTicks);
        for (i, expr) in body.iter().enumerate().rev() {
            let last = i + 1 == body.len();
            if !last {
                steps.push(Step::EmitFlushed {
                    op: Op::Pop,
                    span: expr.span,
                });
            }
            steps.push(Step::Expr {
                expr,
                is_tail: last && is_tail,
            });
        }
    }

    fn run_steps(&mut self, steps: &mut Vec<Step<'a>>) -> Result<(), Error> {
        while let Some(step) = steps.pop() {
            match step {
                Step::Expr { expr, is_tail } => self.step_expr(expr, is_tail, steps)?,
                Step::EmitFlushed { op, span } => {
                    self.emit(op, span);
                }
                Step::EmitRaw { op, span } => {
                    self.chunk().emit(op, span);
                }
                Step::FlushTicks => {
                    self.flush_ticks();
                }
                Step::BindLocal { name, span } => {
                    let slot = self.scopes.last_mut().unwrap().alloc_slot(name, span)?;
                    self.chunk().emit(Op::SetLocal(slot), span);
                }
                Step::RestoreLocals {
                    saved_locals_len,
                    saved_next_slot,
                } => {
                    let scope = self.scopes.last_mut().unwrap();
                    scope.locals.truncate(saved_locals_len);
                    scope.next_slot = saved_next_slot;
                }
                Step::FinishStruct {
                    name,
                    field_names,
                    span,
                } => {
                    let s_idx = self.chunk().add_struct(StructDesc {
                        name: Rc::from(name),
                        fields: field_names,
                    });
                    self.emit(Op::MakeStruct(s_idx), span);
                }
                Step::FinishWith { field_names, span } => {
                    let s_idx = self.chunk().add_struct(StructDesc {
                        name: Rc::from(""),
                        fields: field_names,
                    });
                    self.emit(Op::WithFinish(s_idx), span);
                }
                Step::FinishFn {
                    name,
                    arity,
                    is_defn,
                    span,
                } => {
                    let finished = self.scopes.pop().unwrap();
                    let captures: Vec<Capture> =
                        finished.captures.into_iter().map(|(_, c)| c).collect();
                    let proto = Rc::new(FnProto {
                        name: name.clone(),
                        module: finished.module,
                        arity,
                        local_count: finished.max_slots,
                        captures,
                        chunk: finished.chunk,
                    });
                    let p_idx = self.chunk().add_proto(proto);
                    if is_defn {
                        let n_idx = self.syms.intern(name.as_deref().unwrap());
                        self.emit(Op::DefFn(n_idx, p_idx), span);
                    } else {
                        self.emit(Op::MakeClosure(p_idx), span);
                    }
                }
                Step::IfAfterCond {
                    then_branch,
                    else_branch,
                    is_tail,
                    cond_span,
                    span,
                } => {
                    let j_else = self.emit(Op::JumpIfFalse(0), cond_span);
                    steps.push(Step::IfAfterThen {
                        j_else,
                        else_branch,
                        is_tail,
                        span,
                    });
                    steps.push(Step::Expr {
                        expr: then_branch,
                        is_tail,
                    });
                }
                Step::IfAfterThen {
                    j_else,
                    else_branch,
                    is_tail,
                    span,
                } => {
                    let j_end = self.emit(Op::Jump(0), span);
                    let else_ip = self.chunk().code.len();
                    self.patch_jump(j_else, else_ip);
                    steps.push(Step::PatchJump { op_idx: j_end });
                    steps.push(Step::FlushTicks);
                    steps.push(Step::Expr {
                        expr: else_branch,
                        is_tail,
                    });
                }
                Step::PatchJump { op_idx } => {
                    let end_ip = self.chunk().code.len();
                    self.patch_jump(op_idx, end_ip);
                }
                Step::WhenAfterCond {
                    body,
                    cond_span,
                    span,
                } => {
                    let j_end = self.emit(Op::JumpIfFalse(0), cond_span);
                    steps.push(Step::WhenAfterBody { j_end, span });
                    self.push_body(steps, body, false);
                }
                Step::WhenAfterBody { j_end, span } => {
                    self.emit(Op::Pop, span);
                    let end_ip = self.chunk().code.len();
                    self.patch_jump(j_end, end_ip);
                    self.chunk().emit(Op::Void, span);
                }
                Step::MatchStartArms {
                    arms,
                    idx,
                    end_jumps,
                    is_tail,
                    span,
                } => {
                    if idx >= arms.len() {
                        self.chunk().emit(Op::Pop, span);
                        self.emit(Op::MatchFail, span);
                        let end_ip = self.chunk().code.len();
                        for j in end_jumps {
                            self.patch_jump(j, end_ip);
                        }
                        continue;
                    }
                    let arm = &arms[idx];
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
                    steps.push(Step::MatchAfterArm {
                        arms,
                        idx,
                        try_ip,
                        saved_locals_len,
                        saved_next_slot,
                        end_jumps,
                        is_tail,
                        span,
                    });
                    self.push_body(steps, &arm.body, is_tail);
                }
                Step::MatchAfterArm {
                    arms,
                    idx,
                    try_ip,
                    saved_locals_len,
                    saved_next_slot,
                    mut end_jumps,
                    is_tail,
                    span,
                } => {
                    let arm = &arms[idx];
                    let scope = self.scopes.last_mut().unwrap();
                    scope.locals.truncate(saved_locals_len);
                    scope.next_slot = saved_next_slot;
                    end_jumps.push(self.emit(Op::Jump(0), arm.span));
                    let fail_ip = self.chunk().code.len();
                    self.patch_jump(try_ip, fail_ip);
                    steps.push(Step::MatchStartArms {
                        arms,
                        idx: idx + 1,
                        end_jumps,
                        is_tail,
                        span,
                    });
                }
                Step::ShortCircuitAfterArg {
                    is_and,
                    args,
                    idx,
                    mut jumps,
                    span,
                } => {
                    let arg_span = args[idx].span;
                    let j = if is_and {
                        self.emit(Op::JumpIfFalsePeek(0), arg_span)
                    } else {
                        self.emit(Op::JumpIfTruePeek(0), arg_span)
                    };
                    jumps.push(j);
                    let next_idx = idx + 1;
                    if next_idx < args.len() {
                        steps.push(Step::ShortCircuitAfterArg {
                            is_and,
                            args,
                            idx: next_idx,
                            jumps,
                            span,
                        });
                        steps.push(Step::Expr {
                            expr: &args[next_idx],
                            is_tail: false,
                        });
                    } else {
                        self.chunk().emit(Op::Bool(is_and), span);
                        let end_ip = self.chunk().code.len();
                        for jmp in jumps {
                            self.patch_jump(jmp, end_ip);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn step_expr(
        &mut self,
        expr: &'a Expr,
        is_tail: bool,
        steps: &mut Vec<Step<'a>>,
    ) -> Result<(), Error> {
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
                    steps.push(Step::EmitFlushed {
                        op: Op::MakeList(count),
                        span,
                    });
                    for item in items.iter().rev() {
                        steps.push(Step::Expr {
                            expr: item,
                            is_tail: false,
                        });
                    }
                }
            }
            ExprKind::Tuple(items) => {
                let count = u32::try_from(items.len())
                    .map_err(|_| Error::parse(span, "tuple literal too large"))?;
                steps.push(Step::EmitFlushed {
                    op: Op::MakeTuple(count),
                    span,
                });
                for item in items.iter().rev() {
                    steps.push(Step::Expr {
                        expr: item,
                        is_tail: false,
                    });
                }
            }
            ExprKind::Struct { name, fields } => {
                let field_names: Vec<Rc<str>> =
                    fields.iter().map(|(k, _)| Rc::from(k.as_str())).collect();
                steps.push(Step::FinishStruct {
                    name,
                    field_names,
                    span,
                });
                for (_, fexpr) in fields.iter().rev() {
                    steps.push(Step::Expr {
                        expr: fexpr,
                        is_tail: false,
                    });
                }
            }
            ExprKind::With { target, fields } => {
                let field_names: Vec<Rc<str>> =
                    fields.iter().map(|(k, _)| Rc::from(k.as_str())).collect();
                steps.push(Step::FinishWith { field_names, span });
                for (idx, (fname, fexpr)) in fields.iter().enumerate().rev() {
                    let f_idx = self.chunk().add_string(fname);
                    steps.push(Step::Expr {
                        expr: fexpr,
                        is_tail: false,
                    });
                    steps.push(Step::EmitFlushed {
                        op: Op::WithCheckField(f_idx, idx as u32),
                        span,
                    });
                }
                steps.push(Step::EmitFlushed {
                    op: Op::WithCheckStruct,
                    span,
                });
                steps.push(Step::Expr {
                    expr: target,
                    is_tail: false,
                });
            }
            ExprKind::Variant {
                type_name,
                variant,
                data,
            } => {
                let t_idx = self.chunk().add_string(type_name);
                let v_idx = self.chunk().add_string(variant);
                let op = Op::MakeVariant {
                    type_name: t_idx,
                    variant: v_idx,
                    has_data: data.is_some(),
                };
                if let Some(d) = data {
                    steps.push(Step::EmitFlushed { op, span });
                    steps.push(Step::Expr {
                        expr: d,
                        is_tail: false,
                    });
                } else {
                    self.emit(op, span);
                }
            }
            ExprKind::Some(inner) => {
                steps.push(Step::EmitFlushed {
                    op: Op::MakeSome,
                    span,
                });
                steps.push(Step::Expr {
                    expr: inner,
                    is_tail: false,
                });
            }
            ExprKind::Ok(inner) => {
                steps.push(Step::EmitFlushed {
                    op: Op::MakeOk,
                    span,
                });
                steps.push(Step::Expr {
                    expr: inner,
                    is_tail: false,
                });
            }
            ExprKind::Err(inner) => {
                steps.push(Step::EmitFlushed {
                    op: Op::MakeErr,
                    span,
                });
                steps.push(Step::Expr {
                    expr: inner,
                    is_tail: false,
                });
            }
            ExprKind::Field { target, field } => {
                let f_idx = self.chunk().add_string(field);
                steps.push(Step::EmitFlushed {
                    op: Op::GetField(f_idx),
                    span,
                });
                steps.push(Step::Expr {
                    expr: target,
                    is_tail: false,
                });
            }
            ExprKind::Index { target, index } => {
                steps.push(Step::EmitFlushed {
                    op: Op::GetIndex,
                    span,
                });
                steps.push(Step::Expr {
                    expr: index,
                    is_tail: false,
                });
                steps.push(Step::Expr {
                    expr: target,
                    is_tail: false,
                });
            }
            ExprKind::Def { name, value } => {
                let n_idx = self.syms.intern(name);
                steps.push(Step::EmitFlushed {
                    op: Op::DefGlobal(n_idx),
                    span,
                });
                steps.push(Step::Expr {
                    expr: value,
                    is_tail: false,
                });
            }
            ExprKind::Defn { name, params, body } => {
                let arity = u32::try_from(params.len())
                    .map_err(|_| Error::parse(span, "too many parameters"))?;
                let mut inner = Scope::new(self.module);
                for p in params {
                    inner.alloc_slot(p.clone(), span)?;
                }
                self.scopes.push(inner);
                steps.push(Step::FinishFn {
                    name: Some(name.clone()),
                    arity,
                    is_defn: true,
                    span,
                });
                self.push_body(steps, body, true);
            }
            ExprKind::Fn { params, body } => {
                let arity = u32::try_from(params.len())
                    .map_err(|_| Error::parse(span, "too many parameters"))?;
                let mut inner = Scope::new(self.module);
                for p in params {
                    inner.alloc_slot(p.clone(), span)?;
                }
                self.scopes.push(inner);
                steps.push(Step::FinishFn {
                    name: None,
                    arity,
                    is_defn: false,
                    span,
                });
                self.push_body(steps, body, true);
            }
            ExprKind::Let { bindings, body } => {
                let saved_locals_len = self.scopes.last().unwrap().locals.len();
                let saved_next_slot = self.scopes.last().unwrap().next_slot;
                steps.push(Step::RestoreLocals {
                    saved_locals_len,
                    saved_next_slot,
                });
                self.push_body(steps, body, is_tail);
                for (name, val_expr) in bindings.iter().rev() {
                    steps.push(Step::BindLocal {
                        name: name.clone(),
                        span: val_expr.span,
                    });
                    steps.push(Step::Expr {
                        expr: val_expr,
                        is_tail: false,
                    });
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                steps.push(Step::IfAfterCond {
                    then_branch,
                    else_branch,
                    is_tail,
                    cond_span: cond.span,
                    span,
                });
                steps.push(Step::Expr {
                    expr: cond,
                    is_tail: false,
                });
            }
            ExprKind::When { cond, body } => {
                steps.push(Step::WhenAfterCond {
                    body,
                    cond_span: cond.span,
                    span,
                });
                steps.push(Step::Expr {
                    expr: cond,
                    is_tail: false,
                });
            }
            ExprKind::Match { scrutinee, arms } => {
                steps.push(Step::MatchStartArms {
                    arms,
                    idx: 0,
                    end_jumps: Vec::with_capacity(arms.len()),
                    is_tail,
                    span,
                });
                steps.push(Step::Expr {
                    expr: scrutinee,
                    is_tail: false,
                });
            }
            ExprKind::Call { callee, args } => {
                if let ExprKind::Ident(op) = &callee.kind
                    && (op == "and" || op == "or")
                {
                    let is_and = op == "and";
                    if args.is_empty() {
                        self.emit(if is_and { Op::And } else { Op::Or }, span);
                    } else {
                        steps.push(Step::ShortCircuitAfterArg {
                            is_and,
                            args,
                            idx: 0,
                            jumps: Vec::with_capacity(args.len()),
                            span,
                        });
                        steps.push(Step::Expr {
                            expr: &args[0],
                            is_tail: false,
                        });
                    }
                    return Ok(());
                }
                let argc = u32::try_from(args.len())
                    .map_err(|_| Error::parse(span, "too many arguments"))?;
                let call_op = if is_tail {
                    Op::TailCall(argc)
                } else {
                    Op::Call(argc)
                };
                steps.push(Step::EmitFlushed { op: call_op, span });
                for arg in args.iter().rev() {
                    steps.push(Step::Expr {
                        expr: arg,
                        is_tail: false,
                    });
                }
                steps.push(Step::Expr {
                    expr: callee,
                    is_tail: false,
                });
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
