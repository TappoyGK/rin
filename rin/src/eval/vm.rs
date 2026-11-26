//! バイトコード命令の実行ループとスタックフレーム巻き戻し処理を実装する。

use crate::ast::Pattern;
use crate::bytecode::{BuiltinId, Op};
use crate::diagnostic::{available, suggest};
use crate::error::{Error, Span, StackFrame};
use crate::pool::{Obj, ObjSlice, RVal};

use super::{CallOrigin, FrameSource, Runner};

#[cfg(debug_assertions)]
struct Resume {
    ip: usize,
    stack_start: usize,
    captures: ObjSlice<RVal>,
}

// Release の戻り値は従来どおり RVal。Debug だけ呼び出し前に命令ループを抜ける。
#[cfg(not(debug_assertions))]
type FrameExit = RVal;

#[cfg(debug_assertions)]
enum FrameExit {
    Return(RVal),
    Call {
        arg_start: usize,
        argc: usize,
        span: Span,
    },
}

// Release の命令ループと Debug の小さな呼び出し処理で、同じ後始末を使う。
macro_rules! call_closure {
    ($runner:ident, $id:ident, $callee:ident, $arg_start:ident, $argc:ident, $span:ident, $result:ident => $finish:expr) => {{
        let (proto, cap) = match $runner.pool.get($id) {
            Obj::Closure { proto, captures } => (std::rc::Rc::clone(proto), *captures),
            _ => {
                #[cfg(debug_assertions)]
                return Err($runner.call_type_error($callee, $span));
                #[cfg(not(debug_assertions))]
                return Err(Error::runtime(
                    "expr.call",
                    $span,
                    format!(
                        "cannot call value of type {}",
                        $callee.type_name($runner.pool)
                    ),
                ));
            }
        };
        if proto.arity as usize != $argc {
            #[cfg(debug_assertions)]
            return Err($runner.call_arity_error(proto.arity, $argc, $span));
            #[cfg(not(debug_assertions))]
            return Err(Error::runtime(
                "expr.call",
                $span,
                format!("expected {} arguments, got {}", proto.arity, $argc),
            ));
        }
        $runner.enter_depth($span)?;
        // 呼び出し対象を引数の直前に残し、引数の memmove を避ける。
        let child_base = $arg_start;
        let child_locals = proto.local_count as usize;
        $runner.stack.resize(child_base + child_locals, RVal::Void);
        let prev_mod = std::mem::replace(&mut $runner.active_module, proto.module);
        let $result = $runner.exec_frame(
            FrameSource::Proto(proto),
            cap,
            child_base,
            child_locals,
            Some(CallOrigin::Rin($span)),
            prev_mod,
        );
        $runner.active_module = prev_mod;
        $runner.leave_depth();
        $runner.release_stack_from($arg_start - 1);
        $finish
    }};
}

mod calls;
mod execute;
mod pattern;
mod trace;

impl Runner<'_, '_> {
    pub(super) fn exec_frame(
        &mut self,
        entry: FrameSource<'_>,
        captures: ObjSlice<RVal>,
        base: usize,
        local_count: usize,
        origin: Option<CallOrigin<'_>>,
        caller_module: Option<u32>,
    ) -> Result<RVal, Error> {
        let mut tail_source = None;
        let mut tail_calls = 0;
        #[cfg(debug_assertions)]
        let mut resume = Resume {
            ip: 0,
            stack_start: base + local_count,
            captures,
        };
        #[cfg(not(debug_assertions))]
        let mut res = self.exec_frame_inner(
            &entry,
            &mut tail_source,
            &mut tail_calls,
            captures,
            base,
            local_count,
        );
        #[cfg(debug_assertions)]
        let mut res = loop {
            match self.exec_frame_inner(
                &entry,
                &mut tail_source,
                &mut tail_calls,
                base,
                &mut resume,
            ) {
                Ok(FrameExit::Return(value)) => break Ok(value),
                Ok(FrameExit::Call {
                    arg_start,
                    argc,
                    span,
                }) => match self.call_from_stack(arg_start, argc, span) {
                    Ok(value) => self.stack.push(value),
                    Err(error) => break Err(error),
                },
                Err(error) => break Err(error),
            }
        };
        if let Err(ref mut err) = res {
            self.record_unwind_frame(
                err,
                &entry,
                tail_source.as_ref(),
                tail_calls,
                origin,
                caller_module,
            );
        }
        // キャプチャを直接解放し、状態構造体の drop を経由しない。
        #[cfg(debug_assertions)]
        let Resume {
            captures: _captures,
            ..
        } = resume;
        res
    }
}
