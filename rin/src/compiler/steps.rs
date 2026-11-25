//! 深い構文のコンパイルで使う作業スタックと再開処理。
use super::*;

pub(super) struct IfAfterCondStep<'a> {
    pub(super) then_branch: &'a Expr,
    pub(super) else_branch: &'a Expr,
    pub(super) is_tail: bool,
    pub(super) cond_span: Span,
    pub(super) span: Span,
}

pub(super) struct IfAfterThenStep<'a> {
    pub(super) j_else: usize,
    pub(super) else_branch: &'a Expr,
    pub(super) is_tail: bool,
    pub(super) span: Span,
}

pub(super) struct MatchArmsStep<'a> {
    pub(super) arms: &'a [MatchArm],
    pub(super) idx: usize,
    pub(super) end_jumps: Vec<usize>,
    pub(super) is_tail: bool,
    pub(super) span: Span,
}

pub(super) struct MatchAfterArmStep<'a> {
    pub(super) arms: &'a [MatchArm],
    pub(super) idx: usize,
    pub(super) try_ip: usize,
    pub(super) saved_locals_len: usize,
    pub(super) saved_next_slot: u32,
    pub(super) end_jumps: Vec<usize>,
    pub(super) is_tail: bool,
    pub(super) span: Span,
}

pub(super) struct ShortCircuitStep<'a> {
    pub(super) is_and: bool,
    pub(super) args: &'a [Expr],
    pub(super) idx: usize,
    pub(super) jumps: Vec<usize>,
    pub(super) span: Span,
}

pub(super) struct FinishStructStep<'a> {
    pub(super) name: &'a str,
    pub(super) field_names: Vec<Rc<str>>,
    pub(super) span: Span,
}

pub(super) struct WhenAfterCondStep<'a> {
    pub(super) body: &'a [Expr],
    pub(super) cond_span: Span,
    pub(super) span: Span,
}

pub(super) struct FinishFnStep {
    pub(super) arity: u32,
    pub(super) span: Span,
}

pub(super) enum Step<'a> {
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
        name: &'a str,
        span: Span,
    },
    RestoreLocals {
        saved_locals_len: usize,
        saved_next_slot: u32,
    },
    FinishStruct(Box<FinishStructStep<'a>>),
    FinishWith {
        field_names: Vec<Rc<str>>,
        span: Span,
    },
    FinishFn(Box<FinishFnStep>),
    IfAfterCond(Box<IfAfterCondStep<'a>>),
    IfAfterThen(Box<IfAfterThenStep<'a>>),
    PatchJump {
        op_idx: usize,
    },
    WhenAfterCond(Box<WhenAfterCondStep<'a>>),
    WhenAfterBody {
        j_end: usize,
        span: Span,
    },
    MatchStartArms(Box<MatchArmsStep<'a>>),
    MatchAfterArm(Box<MatchAfterArmStep<'a>>),
    ShortCircuitAfterArg(Box<ShortCircuitStep<'a>>),
}

impl<'a> Compiler<'a> {
    pub(super) fn run_steps(&mut self, steps: &mut Vec<Step<'a>>) -> Result<(), Error> {
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
                    scope.truncate_locals(saved_locals_len);
                    scope.next_slot = saved_next_slot;
                }
                Step::FinishStruct(fs) => {
                    let s_idx = self.chunk().add_struct(StructDesc {
                        name: Rc::from(fs.name),
                        fields: fs.field_names,
                    });
                    self.emit(Op::MakeStruct(s_idx), fs.span);
                }
                Step::FinishWith { field_names, span } => {
                    let s_idx = self.chunk().add_struct(StructDesc {
                        name: Rc::from(""),
                        fields: field_names,
                    });
                    self.emit(Op::WithFinish(s_idx), span);
                }
                Step::FinishFn(ff) => {
                    let finished = self.scopes.pop().unwrap();
                    let captures: Vec<Capture> =
                        finished.captures.into_iter().map(|(_, c)| c).collect();
                    let proto = Rc::new(FnProto {
                        name: None,
                        module: finished.module,
                        arity: ff.arity,
                        local_count: finished.max_slots,
                        captures,
                        chunk: finished.chunk,
                    });
                    let p_idx = self.chunk().add_proto(proto);
                    self.emit(Op::MakeClosure(p_idx), ff.span);
                }
                Step::IfAfterCond(ic) => {
                    let j_else = self.emit(Op::JumpIfFalse(0), ic.cond_span);
                    steps.push(Step::IfAfterThen(Box::new(IfAfterThenStep {
                        j_else,
                        else_branch: ic.else_branch,
                        is_tail: ic.is_tail,
                        span: ic.span,
                    })));
                    steps.push(Step::Expr {
                        expr: ic.then_branch,
                        is_tail: ic.is_tail,
                    });
                }
                Step::IfAfterThen(it) => {
                    let j_end = self.emit(Op::Jump(0), it.span);
                    let else_ip = self.chunk().code.len();
                    self.patch_jump(it.j_else, else_ip);
                    steps.push(Step::PatchJump { op_idx: j_end });
                    steps.push(Step::FlushTicks);
                    steps.push(Step::Expr {
                        expr: it.else_branch,
                        is_tail: it.is_tail,
                    });
                }
                Step::PatchJump { op_idx } => {
                    let end_ip = self.chunk().code.len();
                    self.patch_jump(op_idx, end_ip);
                }
                Step::WhenAfterCond(wc) => {
                    let j_end = self.emit(Op::JumpIfFalse(0), wc.cond_span);
                    steps.push(Step::WhenAfterBody {
                        j_end,
                        span: wc.span,
                    });
                    self.push_body(steps, wc.body, false);
                }
                Step::WhenAfterBody { j_end, span } => {
                    self.emit(Op::Pop, span);
                    let end_ip = self.chunk().code.len();
                    self.patch_jump(j_end, end_ip);
                    self.chunk().emit(Op::Void, span);
                }
                Step::MatchStartArms(ms) => {
                    if ms.idx >= ms.arms.len() {
                        self.chunk().emit(Op::Pop, ms.span);
                        self.emit(Op::MatchFail, ms.span);
                        let end_ip = self.chunk().code.len();
                        for j in ms.end_jumps {
                            self.patch_jump(j, end_ip);
                        }
                        continue;
                    }
                    let arm = &ms.arms[ms.idx];
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
                    steps.push(Step::MatchAfterArm(Box::new(MatchAfterArmStep {
                        arms: ms.arms,
                        idx: ms.idx,
                        try_ip,
                        saved_locals_len,
                        saved_next_slot,
                        end_jumps: ms.end_jumps,
                        is_tail: ms.is_tail,
                        span: ms.span,
                    })));
                    self.push_body(steps, &arm.body, ms.is_tail);
                }
                Step::MatchAfterArm(mut ma) => {
                    let arm = &ma.arms[ma.idx];
                    let scope = self.scopes.last_mut().unwrap();
                    scope.truncate_locals(ma.saved_locals_len);
                    scope.next_slot = ma.saved_next_slot;
                    ma.end_jumps.push(self.emit(Op::Jump(0), arm.span));
                    let fail_ip = self.chunk().code.len();
                    self.patch_jump(ma.try_ip, fail_ip);
                    steps.push(Step::MatchStartArms(Box::new(MatchArmsStep {
                        arms: ma.arms,
                        idx: ma.idx + 1,
                        end_jumps: ma.end_jumps,
                        is_tail: ma.is_tail,
                        span: ma.span,
                    })));
                }
                Step::ShortCircuitAfterArg(mut sc) => {
                    let arg_span = sc.args[sc.idx].span;
                    let j = if sc.is_and {
                        self.emit(Op::JumpIfFalsePeek(0), arg_span)
                    } else {
                        self.emit(Op::JumpIfTruePeek(0), arg_span)
                    };
                    sc.jumps.push(j);
                    let next_idx = sc.idx + 1;
                    if next_idx < sc.args.len() {
                        let next_expr = &sc.args[next_idx];
                        sc.idx = next_idx;
                        steps.push(Step::ShortCircuitAfterArg(sc));
                        steps.push(Step::Expr {
                            expr: next_expr,
                            is_tail: false,
                        });
                    } else {
                        self.chunk().emit(Op::Bool(sc.is_and), sc.span);
                        let end_ip = self.chunk().code.len();
                        for jmp in sc.jumps {
                            self.patch_jump(jmp, end_ip);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
