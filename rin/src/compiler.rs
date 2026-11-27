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

/// `eval_with` / `eval_with_vals` の初期束縛名ビュー（コピーや一時配列確保なし）。
#[derive(Clone, Copy)]
pub(crate) enum InitialBindingNames<'a> {
    #[cfg(feature = "std-value")]
    Values(&'a [(&'a str, crate::Value)]),
    Vals(&'a [(&'a str, crate::Val)]),
}

impl<'a> InitialBindingNames<'a> {
    #[inline]
    pub(crate) fn len(self) -> usize {
        match self {
            #[cfg(feature = "std-value")]
            Self::Values(slice) => slice.len(),
            Self::Vals(slice) => slice.len(),
        }
    }

    #[inline]
    pub(crate) fn is_empty(self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub(crate) fn get(self, idx: usize) -> &'a str {
        match self {
            #[cfg(feature = "std-value")]
            Self::Values(slice) => slice[idx].0,
            Self::Vals(slice) => slice[idx].0,
        }
    }
}

/// `compile_program_with_bindings` が返す、参照された初期束縛の添字列。
pub(crate) enum UsedBindings {
    /// 0..=8 件の初期束縛（参照された束縛のみをスタック上に保持、作業領域確保ゼロ）。
    Small { indices: [u32; 8], len: u8 },
    /// 9 件以上の初期束縛（参照された束縛の添字列、厳密降順）。
    Many(RawVec<u32>),
}

struct InitialBindingState<'a> {
    names: InitialBindingNames<'a>,
    /// `true` のとき `names.len() <= 8` のオンデマンド解決経路を使う。
    lazy_small: bool,
    small_slots: [u32; 8],
    small_used: [u32; 8],
    small_used_len: u8,
    saved_name_head: Option<usize>,
}

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

/// 初期レキシカル束縛名付きで AST 式列をトップレベルのバイトコードチャンクへコンパイルする。
pub(crate) fn compile_program_with_bindings(
    exprs: &[Expr],
    module: Option<u32>,
    syms: &mut SymTable,
    pool: &mut Pool<'_>,
    globals: &EnvSlots,
    frozen: bool,
    initial: InitialBindingNames<'_>,
) -> Result<(Chunk, UsedBindings), ErrRecorded> {
    let initial_len = initial.len();
    if initial_len == 0 {
        let chunk = compile_program_parts(exprs, &[], module, syms, pool, globals, frozen)?;
        return Ok((
            chunk,
            UsedBindings::Small {
                indices: [0; 8],
                len: 0,
            },
        ));
    }
    if initial_len <= 8 {
        let mut state = InitialBindingState {
            names: initial,
            lazy_small: true,
            small_slots: [u32::MAX; 8],
            small_used: [0; 8],
            small_used_len: 0,
            saved_name_head: None,
        };
        let mut root_scope = Scope::new(module);
        // `let` / `match` のスロットは `initial_len` 以降から確保し、オンデマンド割り当てする `0..initial_len` と衝突させない。
        root_scope.next_slot = initial_len as u32;
        let chunk = compile_program_inner(
            exprs,
            &[],
            module,
            syms,
            pool,
            globals,
            frozen,
            root_scope,
            Some(&mut state),
        )?;
        Ok((
            chunk,
            UsedBindings::Small {
                indices: state.small_used,
                len: state.small_used_len,
            },
        ))
    } else {
        compile_program_with_many_bindings(exprs, module, syms, pool, globals, frozen, initial)
    }
}

#[inline(never)]
fn compile_program_with_many_bindings(
    exprs: &[Expr],
    module: Option<u32>,
    syms: &mut SymTable,
    pool: &mut Pool<'_>,
    globals: &EnvSlots,
    frozen: bool,
    initial: InitialBindingNames<'_>,
) -> Result<(Chunk, UsedBindings), ErrRecorded> {
    let mut root_scope = Scope::new(module);
    let mut used_indices = RawVec::<u32>::new();
    let mut idents = collect_expr_idents(exprs, &[], pool)?;
    if !idents.is_empty() {
        // 後ろから走査することで、同名重複の後勝ちと `used_indices` の厳密降順を同時に得る。
        for idx in (0..initial.len()).rev() {
            let name = initial.get(idx);
            let hash = fnv1a_bytes(name.as_bytes());
            if let Some(u32::MAX) = idents.get_by(hash, |k| k.as_str() == name) {
                // SAFETY: `initial` の `&str` はこのコンパイル呼び出し全体を通して有効。
                let raw = unsafe {
                    RawStr::from_raw_utf8(
                        NonNull::new(name.as_ptr().cast_mut()).unwrap_or(NonNull::dangling()),
                        name.len(),
                    )
                };
                let slot = root_scope.alloc_slot(pool, raw, Span::default())?;
                // 既に存在するキーの更新なので確保は起きない。
                let _ =
                    idents.insert_work(&mut pool.work, &mut pool.core, hash, raw, slot, |a, b| {
                        a.as_str() == b.as_str()
                    });
                if used_indices
                    .push_work(&mut pool.work, &mut pool.core, idx as u32)
                    .is_none()
                {
                    return Err(pool.core.err_oom(Span::default()));
                }
            }
        }
    }
    let mut state = InitialBindingState {
        names: initial,
        lazy_small: false,
        small_slots: [u32::MAX; 8],
        small_used: [0; 8],
        small_used_len: 0,
        saved_name_head: None,
    };
    let chunk = compile_program_inner(
        exprs,
        &[],
        module,
        syms,
        pool,
        globals,
        frozen,
        root_scope,
        Some(&mut state),
    )?;
    Ok((chunk, UsedBindings::Many(used_indices)))
}

/// ソース中に現れる識別子（`ExprKind::Ident`）を非再帰で収集する。
fn collect_expr_idents(
    exprs: &[Expr],
    body: &[Expr],
    pool: &mut Pool<'_>,
) -> Result<RawMap<RawStr, u32>, ErrRecorded> {
    let mut idents: RawMap<RawStr, u32> = RawMap::new();
    let mut expr_stack: RawVec<&Expr> = RawVec::new();
    let mut datum_stack: RawVec<&Datum> = RawVec::new();

    for e in exprs.iter().chain(body.iter()) {
        if expr_stack
            .push_work(&mut pool.work, &mut pool.core, e)
            .is_none()
        {
            return Err(pool.core.err_oom(Span::default()));
        }
    }

    fn push_e<'a>(
        stack: &mut RawVec<&'a Expr>,
        pool: &mut Pool<'_>,
        e: &'a Expr,
    ) -> Result<(), ErrRecorded> {
        if stack.push_work(&mut pool.work, &mut pool.core, e).is_none() {
            return Err(pool.core.err_oom(Span::default()));
        }
        Ok(())
    }

    while !expr_stack.is_empty() || !datum_stack.is_empty() {
        while let Some(expr) = expr_stack.pop() {
            match &expr.kind {
                ExprKind::Ident(raw) => {
                    let hash = fnv1a_bytes(raw.as_str().as_bytes());
                    if idents
                        .insert_work(
                            &mut pool.work,
                            &mut pool.core,
                            hash,
                            *raw,
                            u32::MAX,
                            |a, b| a.as_str() == b.as_str(),
                        )
                        .is_none()
                    {
                        return Err(pool.core.err_oom(Span::default()));
                    }
                }
                ExprKind::Int(_)
                | ExprKind::Float(_)
                | ExprKind::Bool(_)
                | ExprKind::Char(_)
                | ExprKind::Str(_)
                | ExprKind::None
                | ExprKind::Import { .. }
                | ExprKind::Export { .. }
                | ExprKind::Quote(_) => {}
                ExprKind::Quasiquote(d) => {
                    if datum_stack
                        .push_work(&mut pool.work, &mut pool.core, d.as_ref())
                        .is_none()
                    {
                        return Err(pool.core.err_oom(Span::default()));
                    }
                }
                ExprKind::Template(items) | ExprKind::List(items) | ExprKind::Tuple(items) => {
                    for item in items.as_slice() {
                        push_e(&mut expr_stack, pool, item)?;
                    }
                }
                ExprKind::Struct(s) => {
                    for (_, val) in s.as_ref().fields.as_slice() {
                        push_e(&mut expr_stack, pool, val)?;
                    }
                }
                ExprKind::Variant(v) => {
                    if let Some(data) = &v.as_ref().data {
                        push_e(&mut expr_stack, pool, data.as_ref())?;
                    }
                }
                ExprKind::Some(e)
                | ExprKind::Ok(e)
                | ExprKind::Err(e)
                | ExprKind::Field { target: e, .. }
                | ExprKind::Def { value: e, .. } => {
                    push_e(&mut expr_stack, pool, e.as_ref())?;
                }
                ExprKind::Index { target, index } => {
                    push_e(&mut expr_stack, pool, target.as_ref())?;
                    push_e(&mut expr_stack, pool, index.as_ref())?;
                }
                ExprKind::With { target, fields } => {
                    push_e(&mut expr_stack, pool, target.as_ref())?;
                    for (_, val) in fields.as_slice() {
                        push_e(&mut expr_stack, pool, val)?;
                    }
                }
                ExprKind::Defn(d) => {
                    for e in d.as_ref().body.as_slice() {
                        push_e(&mut expr_stack, pool, e)?;
                    }
                }
                ExprKind::Fn(f) => {
                    for e in f.as_ref().body.as_slice() {
                        push_e(&mut expr_stack, pool, e)?;
                    }
                }
                ExprKind::Let(l) => {
                    let l = l.as_ref();
                    for (_, val) in l.bindings.as_slice() {
                        push_e(&mut expr_stack, pool, val)?;
                    }
                    for e in l.body.as_slice() {
                        push_e(&mut expr_stack, pool, e)?;
                    }
                }
                ExprKind::If {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    push_e(&mut expr_stack, pool, cond.as_ref())?;
                    push_e(&mut expr_stack, pool, then_branch.as_ref())?;
                    push_e(&mut expr_stack, pool, else_branch.as_ref())?;
                }
                ExprKind::When { cond, body } => {
                    push_e(&mut expr_stack, pool, cond.as_ref())?;
                    for e in body.as_slice() {
                        push_e(&mut expr_stack, pool, e)?;
                    }
                }
                ExprKind::Match { scrutinee, arms } => {
                    push_e(&mut expr_stack, pool, scrutinee.as_ref())?;
                    for arm in arms.as_slice() {
                        for e in arm.body.as_slice() {
                            push_e(&mut expr_stack, pool, e)?;
                        }
                    }
                }
                ExprKind::Call { callee, args } => {
                    push_e(&mut expr_stack, pool, callee.as_ref())?;
                    for arg in args.as_slice() {
                        push_e(&mut expr_stack, pool, arg)?;
                    }
                }
            }
        }
        while let Some(datum) = datum_stack.pop() {
            match &datum.kind {
                DatumKind::List(items) => {
                    for item in items.as_slice() {
                        if datum_stack
                            .push_work(&mut pool.work, &mut pool.core, item)
                            .is_none()
                        {
                            return Err(pool.core.err_oom(Span::default()));
                        }
                    }
                }
                DatumKind::Unquote(e) | DatumKind::UnquoteSplicing(e) => {
                    if expr_stack
                        .push_work(&mut pool.work, &mut pool.core, e.as_ref())
                        .is_none()
                    {
                        return Err(pool.core.err_oom(Span::default()));
                    }
                }
                DatumKind::Int(_)
                | DatumKind::Float(_)
                | DatumKind::Bool(_)
                | DatumKind::Char(_)
                | DatumKind::Str(_)
                | DatumKind::None
                | DatumKind::Ident(_) => {}
            }
        }
    }

    Ok(idents)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_program_parts<'a, 'p>(
    exprs: &'a [Expr],
    body: &'a [Expr],
    module: Option<u32>,
    syms: &'a mut SymTable,
    pool: &'a mut Pool<'p>,
    globals: &'a EnvSlots,
    frozen: bool,
) -> Result<Chunk, ErrRecorded> {
    compile_program_inner(
        exprs,
        body,
        module,
        syms,
        pool,
        globals,
        frozen,
        Scope::new(module),
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn compile_program_inner<'a, 'p>(
    exprs: &'a [Expr],
    body: &'a [Expr],
    module: Option<u32>,
    syms: &'a mut SymTable,
    pool: &'a mut Pool<'p>,
    globals: &'a EnvSlots,
    frozen: bool,
    root_scope: Scope,
    initial: Option<&mut InitialBindingState<'a>>,
) -> Result<Chunk, ErrRecorded> {
    let region = if frozen {
        CodeRegion::Work
    } else {
        CodeRegion::Perm
    };
    let mut scopes = RawVec::new();
    if scopes
        .push_work(&mut pool.work, &mut pool.core, root_scope)
        .is_none()
    {
        return Err(pool.core.err_oom(Span::default()));
    }
    let initial_ptr = initial.map(NonNull::from);
    let mut compiler = Compiler {
        scopes,
        module,
        region,
        syms,
        pool,
        globals,
        frozen,
        name_table: None,
        initial: initial_ptr,
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
    // SAFETY: `initial_ptr` はこの関数内の `initial` への排他的参照から作られ、`compiler` は直後に破棄される。
    let small_used_len = compiler
        .initial
        .map_or(0, |p| unsafe { p.as_ref().small_used_len as u32 });
    let mut finished = compiler.scopes.pop().unwrap();
    finished.chunk.local_count = finished.max_slots.max(small_used_len);
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
        if finished
            .chunk
            .set_str_objs(pool, CodeRegion::Work, objs)
            .is_none()
        {
            return Err(pool.core.err_oom(Span::default()));
        }
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
    initial: Option<NonNull<InitialBindingState<'a>>>,
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
        if u32::try_from(s.len()).is_err() {
            return Err(self.pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                span,
                "text length limit exceeded (4294967295 bytes)",
            ));
        }
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
