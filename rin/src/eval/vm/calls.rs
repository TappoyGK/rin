use super::*;

impl Runner<'_, '_> {
    #[cfg(debug_assertions)]
    pub(super) fn call_from_stack(
        &mut self,
        arg_start: usize,
        argc: usize,
        span: Span,
    ) -> Result<RVal, Error> {
        let callee = self.stack[arg_start - 1];
        match callee {
            RVal::Builtin(b) => self.call_callback_from_stack(b, arg_start, argc, span),
            RVal::Obj(id) => call_closure!(self, id, callee, arg_start, argc, span, res => res),
            other => Err(self.call_type_error(other, span)),
        }
    }

    // コールバック呼び出しは命令ループを抜けてから行う。通常の演算用の
    // 引数処理をここに展開せず、少数の引数だけを保持する。
    #[cfg(debug_assertions)]
    #[inline(never)]
    pub(super) fn call_callback_from_stack(
        &mut self,
        b: BuiltinId,
        arg_start: usize,
        argc: usize,
        span: Span,
    ) -> Result<RVal, Error> {
        if argc > 3 {
            return self.call_callback_with_extra_args(b, arg_start, span);
        }
        let mut args = [RVal::Void; 3];
        args[..argc].copy_from_slice(&self.stack[arg_start..]);
        self.stack.truncate(arg_start - 1);
        let result = self.dispatch_builtin(b, &args[..argc], span);
        for value in &args[..argc] {
            self.pool.release_val(*value);
        }
        result
    }

    #[cfg(debug_assertions)]
    #[cold]
    #[inline(never)]
    pub(super) fn call_callback_with_extra_args(
        &mut self,
        b: BuiltinId,
        arg_start: usize,
        span: Span,
    ) -> Result<RVal, Error> {
        let args: Vec<_> = self.stack.drain(arg_start..).collect();
        self.stack.pop();
        let result = self.dispatch_builtin(b, &args, span);
        self.release_all(args);
        result
    }

    #[cfg_attr(debug_assertions, inline(never))]
    #[cfg_attr(not(debug_assertions), inline(always))]
    pub(in crate::eval) fn call_builtin_from_stack(
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
                if matches!(
                    b,
                    BuiltinId::Add
                        | BuiltinId::Sub
                        | BuiltinId::Mul
                        | BuiltinId::Lt
                        | BuiltinId::Le
                        | BuiltinId::Gt
                        | BuiltinId::Ge
                        | BuiltinId::Eq
                        | BuiltinId::Ne
                ) {
                    self.charge(1, span)?;
                    if matches!(b, BuiltinId::Eq | BuiltinId::Ne) {
                        self.charge(1, span)?;
                    }
                }
                match b {
                    BuiltinId::Add => {
                        return x
                            .checked_add(y)
                            .map(RVal::Int)
                            .ok_or_else(|| Error::runtime("op.arith", span, "i64 overflow"));
                    }
                    BuiltinId::Sub => {
                        return x
                            .checked_sub(y)
                            .map(RVal::Int)
                            .ok_or_else(|| Error::runtime("op.arith", span, "i64 overflow"));
                    }
                    BuiltinId::Mul => {
                        return x
                            .checked_mul(y)
                            .map(RVal::Int)
                            .ok_or_else(|| Error::runtime("op.arith", span, "i64 overflow"));
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
            let res = self.dispatch_builtin(b, &[lhs, rhs], span);
            self.pool.release_val(lhs);
            self.pool.release_val(rhs);
            return res;
        }
        match argc {
            0 => {
                self.stack.truncate(arg_start - 1);
                self.dispatch_builtin(b, &[], span)
            }
            1 => {
                let a = self.stack[arg_start];
                self.stack.truncate(arg_start - 1);
                let res = self.dispatch_builtin(b, &[a], span);
                self.pool.release_val(a);
                res
            }
            3 => {
                let a = self.stack[arg_start];
                let c = self.stack[arg_start + 1];
                let d = self.stack[arg_start + 2];
                self.stack.truncate(arg_start - 1);
                let res = self.dispatch_builtin(b, &[a, c, d], span);
                self.pool.release_val(a);
                self.pool.release_val(c);
                self.pool.release_val(d);
                res
            }
            _ => {
                let args: Vec<RVal> = self.stack.drain(arg_start..).collect();
                self.stack.pop(); // 呼び出し対象を除去
                let res = self.dispatch_builtin(b, &args, span);
                self.release_all(args);
                res
            }
        }
    }
}
