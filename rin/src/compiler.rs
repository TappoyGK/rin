//! AST をスロット解決付きのバイトコードチャンクへコンパイルする。
//!
//! 深い入れ子でもネイティブスタックを消費しないよう、明示的タスクスタックでコンパイルする。

mod lower;
mod scope;
mod shallow;
mod steps;
#[cfg(test)]
mod tests;
use scope::Scope;
use steps::*;

use std::cell::OnceCell;
use std::num::NonZeroUsize;
use std::rc::Rc;

use crate::ast::{Expr, ExprKind, MatchArm, Pattern};
use crate::bytecode::{
    Capture, Chunk, FnProto, NameInfo, NameNode, NameSite, NameTable, Op, StructDesc, SymTable,
};
use crate::diagnostic::needs_name_candidates;
use crate::error::{Error, ErrorKind, Span};
use crate::eval::{EnvSlots, global_slot};
use crate::pool::Pool;

const SHALLOW_COMPILE_DEPTH: usize = 6;

/// AST 式列をトップレベルのバイトコードチャンクへコンパイルする。
pub(crate) fn compile_program(
    exprs: &[Expr],
    module: Option<u32>,
    syms: &mut SymTable,
    pool: &mut Pool<'_>,
    globals: &EnvSlots,
    frozen: bool,
) -> Result<Chunk, Error> {
    compile_program_parts(exprs, &[], module, syms, pool, globals, frozen)
}

pub(crate) fn compile_program_parts<'a, 'p>(
    exprs: &'a [Expr],
    body: &'a [Expr],
    module: Option<u32>,
    syms: &'a mut SymTable,
    pool: &'a mut Pool<'p>,
    globals: &'a EnvSlots,
    frozen: bool,
) -> Result<Chunk, Error> {
    let mut compiler = Compiler {
        scopes: vec![Scope::new(module)],
        module,
        syms,
        pool,
        globals,
        frozen,
        name_table: NameTable::default(),
        shared_names: None,
    };
    if exprs.is_empty() && body.is_empty() {
        compiler.chunk().emit(Op::Void, Span::default());
    } else {
        if !exprs.is_empty() {
            compiler.compile_top_level(exprs)?;
            if !body.is_empty() {
                compiler.emit(Op::Pop, Span::default());
            }
        }
        if !body.is_empty() {
            compiler.compile_top_level(body)?;
        }
        compiler.flush_ticks();
    }
    if let Some(names) = compiler.shared_names.take() {
        names
            .set(compiler.name_table)
            .expect("name nodes are set once");
    }
    let mut finished = compiler.scopes.pop().unwrap();
    finished.chunk.local_count = finished.max_slots;
    Ok(finished.chunk)
}

#[cfg(test)]
pub(crate) fn step_size() -> usize {
    std::mem::size_of::<Step<'_>>()
}

struct Compiler<'a, 'p> {
    scopes: Vec<Scope<'a>>,
    module: Option<u32>,
    syms: &'a mut SymTable,
    pool: &'a mut Pool<'p>,
    globals: &'a EnvSlots,
    frozen: bool,
    name_table: NameTable,
    shared_names: Option<Rc<OnceCell<NameTable>>>,
}

impl<'a, 'p> Compiler<'a, 'p> {
    fn intern_sym(&mut self, name: &str, span: Span) -> Result<u32, Error> {
        self.syms.intern(self.pool, name).ok_or_else(|| {
            Error::new(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "memory limit exceeded",
            )
        })
    }
    fn decl_error(&self, form: &str, span: Span) -> Error {
        if self.frozen {
            Error::parse(
                "env.frozen",
                span,
                format!("'{form}' is not allowed in a frozen environment"),
            )
        } else {
            let msg = format!("'{form}' is only allowed at top level");
            match form {
                "def" => Error::parse("expr.def", span, msg),
                "defn" => Error::parse("expr.defn", span, msg),
                _ => Error::parse("expr.module", span, msg),
            }
        }
    }
    fn chunk(&mut self) -> &mut Chunk {
        &mut self.scopes.last_mut().unwrap().chunk
    }

    fn flush_ticks(&mut self) {
        let top = self.scopes.last_mut().unwrap();
        let end = top.chunk.tick_spans.len();
        if top.pending_tick_start != end {
            let start = top.pending_tick_start;
            let first_span = top.chunk.tick_spans[start];
            top.chunk
                .emit(Op::Tick(start as u32, (end - start) as u32), first_span);
            top.pending_tick_start = end;
        }
    }

    fn emit(&mut self, op: Op, span: Span) -> usize {
        self.flush_ticks();
        let ip = self.chunk().emit(op, span);
        if let Op::GetGlobal(id) = op
            && global_slot(self.globals, id).is_none()
        {
            self.record_name_site(id, ip);
        }
        ip
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
}
fn pattern_binding(pat: &Pattern) -> Option<&str> {
    match pat {
        Pattern::Var(name) => Some(name.as_str()),
        Pattern::Some(Some(name))
        | Pattern::Ok(Some(name))
        | Pattern::Err(Some(name))
        | Pattern::Variant {
            binding: Some(name),
            ..
        } => Some(name.as_str()),
        _ => None,
    }
}
