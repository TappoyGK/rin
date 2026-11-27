//! AST をスロット解決付きのバイトコードチャンクへコンパイルする。
//!
//! 深い入れ子でもネイティブスタックを消費しないよう、明示的タスクスタックでコンパイルする。
//! 一時データは作業領域 (`WorkArena`)、出力チャンクは読み込み段階なら永続領域 (`PermArena`)、
//! 凍結後の評価なら作業領域 (`WorkArena`) へ配置し、システムヒープを一切使わない。

mod lower;
mod scope;
mod shallow;
mod steps;
#[cfg(all(test, feature = "std-value"))]
mod tests;
use scope::Scope;
use steps::*;

use std::num::NonZeroU32;
use std::ptr::NonNull;

use crate::ast::{Datum, DatumKind, Expr, ExprKind, MatchArm, Pattern};
use crate::bytecode::{
    Capture, Chunk, FnProto, FnProtoPtr, NameInfo, NameNode, NameSite, NameTable, Op, StructDesc,
    SymTable,
};
use crate::diagnostic::needs_name_candidates;
use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::eval::{EnvSlots, global_slot};
use crate::mem::{RawMap, RawStr, RawVec, WorkBox, fnv1a_bytes};
use crate::pool::{CodeRegion, Pool};

const SHALLOW_COMPILE_DEPTH: usize = 3;

/// AST 式列をトップレベルのバイトコードチャンクへコンパイルする。
pub(crate) fn compile_program(
    exprs: &[Expr],
    module: Option<u32>,
    syms: &mut SymTable,
    pool: &mut Pool<'_>,
    globals: &EnvSlots,
    frozen: bool,
) -> Result<Chunk, ErrRecorded> {
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
) -> Result<Chunk, ErrRecorded> {
    let region = if frozen {
        CodeRegion::Work
    } else {
        CodeRegion::Perm
    };
    let mut scopes = RawVec::new();
    if scopes
        .push_work(&mut pool.work, &mut pool.core, Scope::new(module))
        .is_none()
    {
        return Err(pool.core.err_oom(Span::default()));
    }
    let mut compiler = Compiler {
        scopes,
        module,
        region,
        syms,
        pool,
        globals,
        frozen,
        name_table: None,
    };
    if exprs.is_empty() && body.is_empty() {
        compiler.emit_raw(Op::Void, Span::default())?;
    } else {
        if !exprs.is_empty() {
            compiler.compile_top_level(exprs)?;
            if !body.is_empty() {
                compiler.emit(Op::Pop, Span::default())?;
            }
        }
        if !body.is_empty() {
            compiler.compile_top_level(body)?;
        }
        compiler.flush_ticks(Span::default())?;
    }
    let mut finished = compiler.scopes.pop().unwrap();
    finished.chunk.local_count = finished.max_slots;
    if !frozen && !finished.chunk.strings.is_empty() {
        let mut objs = RawVec::new();
        for &s in finished.chunk.strings.as_slice() {
            let Some(raw) = RawStr::from_perm(&mut pool.perm, &mut pool.core, s.as_str()) else {
                return Err(pool.core.err_oom(Span::default()));
            };
            let Some(id) = pool.intern_perm_str(raw) else {
                return Err(pool.core.err_oom(Span::default()));
            };
            if objs.push_work(&mut pool.work, &mut pool.core, id).is_none() {
                return Err(pool.core.err_oom(Span::default()));
            }
        }
        finished.chunk.str_objs = objs;
    }
    Ok(finished.chunk)
}

#[cfg(all(test, feature = "std-value"))]
pub(crate) fn step_size() -> usize {
    std::mem::size_of::<Step<'_>>()
}

type Failed = ErrRecorded;

struct Compiler<'a, 'p> {
    scopes: RawVec<Scope>,
    module: Option<u32>,
    region: CodeRegion,
    syms: &'a mut SymTable,
    pool: &'a mut Pool<'p>,
    globals: &'a EnvSlots,
    frozen: bool,
    name_table: Option<NonNull<NameTable>>,
}

impl<'a, 'p> Compiler<'a, 'p> {
    #[cold]
    fn fail_parse(&mut self, rule: &'static str, span: Span, msg: &str) -> Failed {
        self.pool.core.err_str(rule, ErrorKind::Parse, span, msg)
    }

    #[cold]
    fn fail_oom(&mut self, span: Span) -> Failed {
        self.pool.core.err_oom(span)
    }

    fn intern_sym(&mut self, name: &str, span: Span) -> Result<u32, Failed> {
        match self.syms.intern(self.pool, name) {
            Some(id) => Ok(id),
            None => Err(self.fail_oom(span)),
        }
    }

    fn alloc_step_box<T>(&mut self, val: T, span: Span) -> Result<WorkBox<T>, Failed> {
        match WorkBox::new(&mut self.pool.work, &mut self.pool.core, val) {
            Some(b) => Ok(b),
            None => Err(self.fail_oom(span)),
        }
    }

    fn push_step(
        &mut self,
        steps: &mut RawVec<Step<'a>>,
        step: Step<'a>,
        span: Span,
    ) -> Result<(), Failed> {
        if steps
            .push_work(&mut self.pool.work, &mut self.pool.core, step)
            .is_none()
        {
            return Err(self.fail_oom(span));
        }
        Ok(())
    }

    fn push_scope(&mut self, scope: Scope, span: Span) -> Result<(), Failed> {
        if self
            .scopes
            .push_work(&mut self.pool.work, &mut self.pool.core, scope)
            .is_none()
        {
            return Err(self.fail_oom(span));
        }
        Ok(())
    }

    #[cold]
    fn fail_decl(&mut self, form: &str, span: Span) -> Failed {
        if self.frozen {
            self.pool.core.err_fmt(
                "env.frozen",
                ErrorKind::Parse,
                span,
                format_args!("'{form}' is not allowed in a frozen environment"),
            )
        } else {
            let rule = match form {
                "def" => "expr.def",
                "defn" => "expr.defn",
                _ => "expr.module",
            };
            self.pool.core.err_fmt(
                rule,
                ErrorKind::Parse,
                span,
                format_args!("'{form}' is only allowed at top level"),
            )
        }
    }

    fn chunk(&mut self) -> &mut Chunk {
        &mut self.scopes.last_mut().unwrap().chunk
    }

    fn current_region(&self) -> CodeRegion {
        // コンパイル中は作業領域で構築し、`alloc_proto` で永続領域へ余剰容量ゼロでコピーする。
        CodeRegion::Work
    }

    fn push_tick_span(&mut self, span: Span) -> Result<(), Failed> {
        let region = self.current_region();
        let top = self.scopes.last_mut().unwrap();
        if self
            .pool
            .push_code(region, &mut top.chunk.tick_spans, span)
            .is_none()
        {
            return Err(self.fail_oom(span));
        }
        Ok(())
    }

    fn emit_raw(&mut self, op: Op, span: Span) -> Result<usize, Failed> {
        let region = self.current_region();
        let top = self.scopes.last_mut().unwrap();
        match top.chunk.emit(self.pool, region, op, span) {
            Some(ip) => Ok(ip),
            None => Err(self.fail_oom(span)),
        }
    }

    fn add_string(&mut self, s: &str, span: Span) -> Result<u32, Failed> {
        let region = self.current_region();
        let top = self.scopes.last_mut().unwrap();
        match top
            .chunk
            .add_string(self.pool, region, &mut top.string_map, s)
        {
            Some(idx) => Ok(idx),
            None => Err(self.fail_oom(span)),
        }
    }

    fn add_float(&mut self, x: f64, span: Span) -> Result<u32, Failed> {
        let region = self.current_region();
        let top = self.scopes.last_mut().unwrap();
        match top.chunk.add_float(self.pool, region, x) {
            Some(idx) => Ok(idx),
            None => Err(self.fail_oom(span)),
        }
    }

    fn add_struct(
        &mut self,
        name: &str,
        fields: RawVec<RawStr>,
        span: Span,
    ) -> Result<u32, Failed> {
        let region = self.current_region();
        let Some(name_raw) = self.pool.alloc_code_str(region, name) else {
            return Err(self.fail_oom(span));
        };
        let desc = StructDesc {
            name: name_raw,
            fields,
        };
        let top = self.scopes.last_mut().unwrap();
        match top.chunk.add_struct(self.pool, region, desc) {
            Some(idx) => Ok(idx),
            None => Err(self.fail_oom(span)),
        }
    }

    fn build_field_names(
        &mut self,
        fields: &[(RawStr, Expr)],
        span: Span,
    ) -> Result<RawVec<RawStr>, Failed> {
        let region = self.current_region();
        let mut out = RawVec::new();
        for (k, _) in fields {
            let Some(raw) = self.pool.alloc_code_str(region, k.as_str()) else {
                return Err(self.fail_oom(span));
            };
            if self.pool.push_code(region, &mut out, raw).is_none() {
                return Err(self.fail_oom(span));
            }
        }
        Ok(out)
    }

    fn add_pattern(&mut self, pat: Pattern, span: Span) -> Result<u32, Failed> {
        let region = self.current_region();
        let top = self.scopes.last_mut().unwrap();
        match top.chunk.add_pattern(self.pool, region, pat) {
            Some(idx) => Ok(idx),
            None => Err(self.fail_oom(span)),
        }
    }

    fn add_splice_mask(&mut self, mask: RawVec<u8>, span: Span) -> Result<u32, Failed> {
        let region = self.current_region();
        let top = self.scopes.last_mut().unwrap();
        match top.chunk.add_splice_mask(self.pool, region, mask) {
            Some(idx) => Ok(idx),
            None => Err(self.fail_oom(span)),
        }
    }

    #[inline(never)]
    fn alloc_proto(
        &mut self,
        finished: &Scope,
        name: Option<&str>,
        arity: u32,
        span: Span,
    ) -> Result<FnProtoPtr, Failed> {
        let proto_region = self.region;
        let name_raw = match name {
            Some(s) => match self.pool.alloc_code_str(proto_region, s) {
                Some(raw) => Some(raw),
                None => return Err(self.fail_oom(span)),
            },
            None => None,
        };
        let mut captures = RawVec::new();
        for &(_, c) in finished.captures.as_slice() {
            if self
                .pool
                .push_code(proto_region, &mut captures, c)
                .is_none()
            {
                return Err(self.fail_oom(span));
            }
        }
        let chunk = if proto_region == CodeRegion::Perm {
            match finished.chunk.copy_to_perm(self.pool) {
                Some(c) => c,
                None => return Err(self.fail_oom(span)),
            }
        } else {
            finished.chunk
        };
        let proto = FnProto {
            name: name_raw,
            module: finished.module,
            arity,
            local_count: finished.max_slots,
            captures,
            chunk,
        };
        let Some(ptr) = self.pool.alloc_code_val(proto_region, proto) else {
            return Err(self.fail_oom(span));
        };
        Ok(FnProtoPtr::new(ptr))
    }

    fn add_proto_ptr(&mut self, proto: FnProtoPtr, span: Span) -> Result<u32, Failed> {
        let chunk_region = self.current_region();
        let top = self.scopes.last_mut().unwrap();
        match top.chunk.add_proto(self.pool, chunk_region, proto) {
            Some(idx) => Ok(idx),
            None => Err(self.fail_oom(span)),
        }
    }

    #[inline(never)]
    fn finish_proto(
        &mut self,
        finished: &Scope,
        name: Option<&str>,
        arity: u32,
        span: Span,
    ) -> Result<u32, Failed> {
        let proto = self.alloc_proto(finished, name, arity, span)?;
        self.add_proto_ptr(proto, span)
    }

    fn flush_ticks(&mut self, span: Span) -> Result<(), Failed> {
        let region = self.current_region();
        let top = self.scopes.last_mut().unwrap();
        let end = top.chunk.tick_spans.len();
        if top.pending_tick_start != end {
            let start = top.pending_tick_start;
            let first_span = top.chunk.tick_spans[start];
            if top
                .chunk
                .emit(
                    self.pool,
                    region,
                    Op::Tick(start as u32, (end - start) as u32),
                    first_span,
                )
                .is_none()
            {
                return Err(self.fail_oom(span));
            }
            top.pending_tick_start = end;
        }
        Ok(())
    }

    fn emit(&mut self, op: Op, span: Span) -> Result<usize, Failed> {
        self.flush_ticks(span)?;
        let ip = self.emit_raw(op, span)?;
        if let Op::GetGlobal(id) = op
            && global_slot(self.globals, id).is_none()
        {
            self.record_name_site(id, ip, span)?;
        }
        Ok(ip)
    }

    fn patch_jump(&mut self, op_idx: usize, target: usize) {
        let target = u32::try_from(target).expect("bytecode chunk exceeds u32::MAX");
        let code = self.chunk().code.as_mut_slice();
        match &mut code[op_idx] {
            Op::Jump(t) | Op::JumpIfFalse(t) | Op::JumpIfFalsePeek(t) | Op::JumpIfTruePeek(t) => {
                *t = target
            }
            Op::TryMatch { fail_ip, .. } => *fail_ip = target,
            _ => unreachable!(),
        }
    }

    fn push_body(
        &mut self,
        steps: &mut RawVec<Step<'a>>,
        body: &'a [Expr],
        is_tail: bool,
        span: Span,
    ) -> Result<(), Failed> {
        if body.is_empty() {
            self.push_step(
                steps,
                Step::EmitRaw {
                    op: Op::Void,
                    span: Span::default(),
                },
                span,
            )?;
            return Ok(());
        }
        self.push_step(steps, Step::FlushTicks, span)?;
        for (i, expr) in body.iter().enumerate().rev() {
            let last = i + 1 == body.len();
            if !last {
                self.push_step(
                    steps,
                    Step::EmitFlushed {
                        op: Op::Pop,
                        span: expr.span,
                    },
                    expr.span,
                )?;
            }
            self.push_step(
                steps,
                Step::Expr {
                    expr,
                    is_tail: last && is_tail,
                },
                expr.span,
            )?;
        }
        Ok(())
    }
}

fn pattern_binding(pat: &Pattern) -> Option<RawStr> {
    match *pat {
        Pattern::Var(name) => Some(name),
        Pattern::Some(Some(name))
        | Pattern::Ok(Some(name))
        | Pattern::Err(Some(name))
        | Pattern::Variant {
            binding: Some(name),
            ..
        } => Some(name),
        _ => None,
    }
}
