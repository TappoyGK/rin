//! 深い構文のコンパイルで使う作業スタックと再開処理（`WorkArena` 完結）。
use super::*;

#[derive(Clone, Copy)]
pub(super) struct IfAfterCondStep<'a> {
    pub(super) then_branch: &'a Expr,
    pub(super) else_branch: &'a Expr,
    pub(super) is_tail: bool,
    pub(super) cond_span: Span,
    pub(super) span: Span,
}

#[derive(Clone, Copy)]
pub(super) struct IfAfterThenStep<'a> {
    pub(super) j_else: usize,
    pub(super) else_branch: &'a Expr,
    pub(super) is_tail: bool,
    pub(super) span: Span,
}

#[derive(Clone, Copy)]
pub(super) struct MatchArmsStep<'a> {
    pub(super) arms: &'a [MatchArm],
    pub(super) idx: usize,
    pub(super) end_jumps: RawVec<usize>,
    pub(super) is_tail: bool,
    pub(super) span: Span,
}

#[derive(Clone, Copy)]
pub(super) struct MatchAfterArmStep<'a> {
    pub(super) arms: &'a [MatchArm],
    pub(super) idx: usize,
    pub(super) try_ip: usize,
    pub(super) saved_locals_len: usize,
    pub(super) saved_next_slot: u32,
    pub(super) end_jumps: RawVec<usize>,
    pub(super) is_tail: bool,
    pub(super) span: Span,
}

#[derive(Clone, Copy)]
pub(super) struct ShortCircuitStep<'a> {
    pub(super) is_and: bool,
    pub(super) args: &'a [Expr],
    pub(super) idx: usize,
    pub(super) jumps: RawVec<usize>,
    pub(super) span: Span,
}

#[derive(Clone, Copy)]
pub(super) struct FinishStructStep {
    pub(super) name: RawStr,
    pub(super) field_names: RawVec<RawStr>,
    pub(super) span: Span,
}

#[derive(Clone, Copy)]
pub(super) struct WhenAfterCondStep<'a> {
    pub(super) body: &'a [Expr],
    pub(super) cond_span: Span,
    pub(super) span: Span,
}

#[derive(Clone, Copy)]
pub(super) struct FinishFnStep {
    pub(super) arity: u32,
    pub(super) span: Span,
}

#[derive(Clone, Copy)]
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
        name: RawStr,
        span: Span,
    },
    RestoreLocals {
        saved_locals_len: usize,
        saved_next_slot: u32,
    },
    FinishStruct(WorkBox<FinishStructStep>),
    FinishWith {
        field_names: RawVec<RawStr>,
        span: Span,
    },
    FinishFn(WorkBox<FinishFnStep>),
    IfAfterCond(WorkBox<IfAfterCondStep<'a>>),
    IfAfterThen(WorkBox<IfAfterThenStep<'a>>),
    PatchJump {
        op_idx: usize,
    },
    WhenAfterCond(WorkBox<WhenAfterCondStep<'a>>),
    WhenAfterBody {
        j_end: usize,
        span: Span,
    },
    MatchStartArms(WorkBox<MatchArmsStep<'a>>),
    MatchAfterArm(WorkBox<MatchAfterArmStep<'a>>),
    ShortCircuitAfterArg(WorkBox<ShortCircuitStep<'a>>),
}

impl<'a, 'p> Compiler<'a, 'p> {
    pub(super) fn run_steps(&mut self, steps: &mut RawVec<Step<'a>>) -> Result<(), Failed> {
        while let Some(step) = steps.pop() {
            match step {
                Step::Expr { expr, is_tail } => self.step_expr(expr, is_tail, steps)?,
                Step::EmitFlushed { op, span } => {
                    self.emit(op, span)?;
                }
                Step::EmitRaw { op, span } => {
                    self.emit_raw(op, span)?;
                }
                Step::FlushTicks => {
                    self.flush_ticks(Span::default())?;
                }
                Step::BindLocal { name, span } => {
                    let slot = match self
                        .scopes
                        .last_mut()
                        .unwrap()
                        .alloc_slot(self.pool, name, span)
                    {
                        Ok(s) => s,
                        Err(e) => return Err(self.fail(e)),
                    };
                    self.emit_raw(Op::SetLocal(slot), span)?;
                }
                Step::RestoreLocals {
                    saved_locals_len,
                    saved_next_slot,
                } => {
                    let scope = self.scopes.last_mut().unwrap();
                    scope.truncate_locals(self.pool, saved_locals_len);
                    scope.next_slot = saved_next_slot;
                }
                Step::FinishStruct(fs) => {
                    let fs = *fs.as_ref();
                    let s_idx = self.add_struct(fs.name.as_str(), fs.field_names, fs.span)?;
                    self.emit(Op::MakeStruct(s_idx), fs.span)?;
                }
                Step::FinishWith { field_names, span } => {
                    let s_idx = self.add_struct("", field_names, span)?;
                    self.emit(Op::WithFinish(s_idx), span)?;
                }
                Step::FinishFn(ff) => {
                    let ff = *ff.as_ref();
                    let finished = self.scopes.pop().unwrap();
                    let p_idx = self.finish_proto(&finished, None, ff.arity, ff.span)?;
                    self.emit(Op::MakeClosure(p_idx), ff.span)?;
                }
                Step::IfAfterCond(ic) => {
                    let ic = *ic.as_ref();
                    let j_else = self.emit(Op::JumpIfFalse(0), ic.cond_span)?;
                    let step_box = self.alloc_step_box(
                        IfAfterThenStep {
                            j_else,
                            else_branch: ic.else_branch,
                            is_tail: ic.is_tail,
                            span: ic.span,
                        },
                        ic.span,
                    )?;
                    self.push_step(steps, Step::IfAfterThen(step_box), ic.span)?;
                    self.push_step(
                        steps,
                        Step::Expr {
                            expr: ic.then_branch,
                            is_tail: ic.is_tail,
                        },
                        ic.span,
                    )?;
                }
                Step::IfAfterThen(it) => {
                    let it = *it.as_ref();
                    let j_end = self.emit(Op::Jump(0), it.span)?;
                    let else_ip = self.chunk().code.len();
                    self.patch_jump(it.j_else, else_ip);
                    self.push_step(steps, Step::PatchJump { op_idx: j_end }, it.span)?;
                    self.push_step(steps, Step::FlushTicks, it.span)?;
                    self.push_step(
                        steps,
                        Step::Expr {
                            expr: it.else_branch,
                            is_tail: it.is_tail,
                        },
                        it.span,
                    )?;
                }
                Step::PatchJump { op_idx } => {
                    let end_ip = self.chunk().code.len();
                    self.patch_jump(op_idx, end_ip);
                }
                Step::WhenAfterCond(wc) => {
                    let wc = *wc.as_ref();
                    let j_end = self.emit(Op::JumpIfFalse(0), wc.cond_span)?;
                    self.push_step(
                        steps,
                        Step::WhenAfterBody {
                            j_end,
                            span: wc.span,
                        },
                        wc.span,
                    )?;
                    self.push_body(steps, wc.body, false, wc.span)?;
                }
                Step::WhenAfterBody { j_end, span } => {
                    self.emit(Op::Pop, span)?;
                    let end_ip = self.chunk().code.len();
                    self.patch_jump(j_end, end_ip);
                    self.emit_raw(Op::Void, span)?;
                }
                Step::MatchStartArms(ms) => {
                    let ms = *ms.as_ref();
                    if ms.idx >= ms.arms.len() {
                        self.emit_raw(Op::Pop, ms.span)?;
                        self.emit(Op::MatchFail, ms.span)?;
                        let end_ip = self.chunk().code.len();
                        for &j in ms.end_jumps.as_slice() {
                            self.patch_jump(j, end_ip);
                        }
                        continue;
                    }
                    let arm = &ms.arms[ms.idx];
                    let pat_idx = self.add_pattern(arm.pattern, arm.span)?;
                    let try_ip = self.emit(
                        Op::TryMatch {
                            pat_idx,
                            fail_ip: 0,
                        },
                        arm.span,
                    )?;
                    let saved_locals_len = self.scopes.last().unwrap().locals.len();
                    let saved_next_slot = self.scopes.last().unwrap().next_slot;
                    if let Some(bind_name) = pattern_binding(&arm.pattern) {
                        let slot = match self
                            .scopes
                            .last_mut()
                            .unwrap()
                            .alloc_slot(self.pool, bind_name, arm.span)
                        {
                            Ok(s) => s,
                            Err(e) => return Err(self.fail(e)),
                        };
                        self.emit_raw(Op::SetLocal(slot), arm.span)?;
                    }
                    self.emit_raw(Op::Pop, arm.span)?;
                    let step_box = self.alloc_step_box(
                        MatchAfterArmStep {
                            arms: ms.arms,
                            idx: ms.idx,
                            try_ip,
                            saved_locals_len,
                            saved_next_slot,
                            end_jumps: ms.end_jumps,
                            is_tail: ms.is_tail,
                            span: ms.span,
                        },
                        arm.span,
                    )?;
                    self.push_step(steps, Step::MatchAfterArm(step_box), arm.span)?;
                    self.push_body(steps, arm.body.as_slice(), ms.is_tail, arm.span)?;
                }
                Step::MatchAfterArm(mut ma) => {
                    let ma_val = *ma.as_ref();
                    let arm = &ma_val.arms[ma_val.idx];
                    let scope = self.scopes.last_mut().unwrap();
                    scope.truncate_locals(self.pool, ma_val.saved_locals_len);
                    scope.next_slot = ma_val.saved_next_slot;
                    let j = self.emit(Op::Jump(0), arm.span)?;
                    if ma
                        .as_mut()
                        .end_jumps
                        .push_work(&mut self.pool.work, &mut self.pool.core, j)
                        .is_none()
                    {
                        return Err(self.fail_oom(arm.span));
                    }
                    let fail_ip = self.chunk().code.len();
                    self.patch_jump(ma_val.try_ip, fail_ip);
                    let end_jumps = ma.as_ref().end_jumps;
                    let step_box = self.alloc_step_box(
                        MatchArmsStep {
                            arms: ma_val.arms,
                            idx: ma_val.idx + 1,
                            end_jumps,
                            is_tail: ma_val.is_tail,
                            span: ma_val.span,
                        },
                        arm.span,
                    )?;
                    self.push_step(steps, Step::MatchStartArms(step_box), arm.span)?;
                }
                Step::ShortCircuitAfterArg(mut sc) => {
                    let sc_val = *sc.as_ref();
                    let arg_span = sc_val.args[sc_val.idx].span;
                    let j = if sc_val.is_and {
                        self.emit(Op::JumpIfFalsePeek(0), arg_span)?
                    } else {
                        self.emit(Op::JumpIfTruePeek(0), arg_span)?
                    };
                    if sc
                        .as_mut()
                        .jumps
                        .push_work(&mut self.pool.work, &mut self.pool.core, j)
                        .is_none()
                    {
                        return Err(self.fail_oom(arg_span));
                    }
                    let next_idx = sc_val.idx + 1;
                    if next_idx < sc_val.args.len() {
                        let next_expr = &sc_val.args[next_idx];
                        sc.as_mut().idx = next_idx;
                        self.push_step(steps, Step::ShortCircuitAfterArg(sc), arg_span)?;
                        self.push_step(
                            steps,
                            Step::Expr {
                                expr: next_expr,
                                is_tail: false,
                            },
                            arg_span,
                        )?;
                    } else {
                        self.emit_raw(Op::Bool(sc_val.is_and), sc_val.span)?;
                        let end_ip = self.chunk().code.len();
                        for &jmp in sc.as_ref().jumps.as_slice() {
                            self.patch_jump(jmp, end_ip);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
