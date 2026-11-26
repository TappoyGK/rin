use super::*;
use crate::mem::RawVec;

impl Runner<'_, '_> {
    // 整数の速い経路だけをVMへ展開し、汎用の引数処理は外に置く。
    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(in crate::eval) fn call_builtin_from_stack(
        &mut self,
        b: BuiltinId,
        arg_start: usize,
        argc: usize,
        chunk: &crate::bytecode::Chunk,
        op_ip: usize,
    ) -> Result<(), ErrRecorded> {
        if argc == 2 {
            let s = self.stack.as_slice();
            let lhs = s[arg_start];
            let rhs = s[arg_start + 1];
            if let (RVal::Int(x), RVal::Int(y)) = (lhs, rhs) {
                let (value, cost) = match b {
                    BuiltinId::Add => (x.checked_add(y).map(RVal::Int), 1),
                    BuiltinId::Sub => (x.checked_sub(y).map(RVal::Int), 1),
                    BuiltinId::Mul => (x.checked_mul(y).map(RVal::Int), 1),
                    BuiltinId::Lt => (Some(RVal::Bool(x < y)), 1),
                    BuiltinId::Le => (Some(RVal::Bool(x <= y)), 1),
                    BuiltinId::Gt => (Some(RVal::Bool(x > y)), 1),
                    BuiltinId::Ge => (Some(RVal::Bool(x >= y)), 1),
                    BuiltinId::Eq => (Some(RVal::Bool(x == y)), 2),
                    BuiltinId::Ne => (Some(RVal::Bool(x != y)), 2),
                    _ => return self.call_builtin_slow(b, arg_start, argc, chunk.spans[op_ip]),
                };
                if !self.try_charge(cost) {
                    self.stack.truncate(arg_start - 1);
                    return self.step_limit_result(chunk.spans[op_ip]);
                }
                return match value {
                    Some(value) => {
                        self.stack.as_mut_slice()[arg_start - 1] = value;
                        self.stack.truncate(arg_start);
                        Ok(())
                    }
                    None => {
                        self.stack.truncate(arg_start - 1);
                        self.integer_overflow(chunk.spans[op_ip]).map(|_| ())
                    }
                };
            }
        }
        self.call_builtin_slow(b, arg_start, argc, chunk.spans[op_ip])
    }

    #[inline(never)]
    fn call_builtin_slow(
        &mut self,
        b: BuiltinId,
        arg_start: usize,
        argc: usize,
        span: Span,
    ) -> Result<(), ErrRecorded> {
        if !self.try_charge(1) {
            self.release_stack_from(arg_start - 1);
            return self.step_limit_result(span);
        }
        let res = if argc == 2 {
            let s = self.stack.as_slice();
            let lhs = s[arg_start];
            let rhs = s[arg_start + 1];
            self.stack.truncate(arg_start - 1);
            let res = self.apply_non_callback_builtin(b, &[lhs, rhs], span);
            self.pool.release_val(lhs);
            self.pool.release_val(rhs);
            res
        } else {
            match argc {
                0 => {
                    self.stack.truncate(arg_start - 1);
                    self.apply_non_callback_builtin(b, &[], span)
                }
                1 => {
                    let a = self.stack.as_slice()[arg_start];
                    self.stack.truncate(arg_start - 1);
                    let res = self.apply_non_callback_builtin(b, &[a], span);
                    self.pool.release_val(a);
                    res
                }
                3 => {
                    let s = self.stack.as_slice();
                    let a = s[arg_start];
                    let c = s[arg_start + 1];
                    let d = s[arg_start + 2];
                    self.stack.truncate(arg_start - 1);
                    let res = self.apply_non_callback_builtin(b, &[a, c, d], span);
                    self.pool.release_val(a);
                    self.pool.release_val(c);
                    self.pool.release_val(d);
                    res
                }
                _ => {
                    let mark = self.pool.work.mark(&self.pool.core);
                    let mut args = RawVec::<RVal>::new();
                    if args
                        .reserve_work(&mut self.pool.work, &mut self.pool.core, argc)
                        .is_none()
                    {
                        self.pool.work.rewind(&mut self.pool.core, mark);
                        self.release_stack_from(arg_start - 1);
                        return Err(self.work_oom(span));
                    }
                    let src = &self.stack.as_slice()[arg_start..arg_start + argc];
                    let _ =
                        args.extend_from_slice_work(&mut self.pool.work, &mut self.pool.core, src);
                    self.stack.truncate(arg_start - 1);
                    let res = self.apply_non_callback_builtin(b, args.as_slice(), span);
                    self.release_slice(args.as_slice());
                    self.pool.work.rewind(&mut self.pool.core, mark);
                    res
                }
            }
        }?;
        self.stack_push(res, span)
    }
}

impl Runner<'_, '_> {
    #[cold]
    #[inline(never)]
    fn integer_overflow(&mut self, span: Span) -> Result<RVal, ErrRecorded> {
        Err(self.err_runtime("op.arith", span, "i64 overflow"))
    }
}
