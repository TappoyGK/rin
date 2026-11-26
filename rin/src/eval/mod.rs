//! バイトコード VM の実行環境 `Builder`・`Machine` と `Runner` を定義する。

mod builtins;
mod host_vals;
mod machine;
mod modules;
mod ops;
mod runner;
mod vm;

use crate::alloc::{Allocator, HeapAllocator};
use crate::bytecode::{
    BuiltinId, Capture, Chunk, FnProtoPtr, PI_SYM_ID, STATIC_SYM_COUNT, SymTable, static_sym_name,
};
use crate::compiler::{compile_program, compile_program_parts};
use crate::diagnostic::{write_available, write_suggest};
use crate::error::{ErrRecorded, Error, ErrorKind, ErrorRef, Limits, Span};
use crate::math::MathFma;
use crate::mem::{RawMap, RawStr, RawVec, fnv1a_bytes, fnv1a_u32};
use crate::parser::parse_program_work;
use crate::pool::{Obj, Pool, RVal};
use crate::value::Value;
use crate::value_ref::{Val, ValueRef};

/// 既定の `HeapAllocator` を用いて Rin ソースを評価する。
///
/// 読み込み段階の環境で 1 回評価する（`def`・`defn` 等も書ける）。
pub fn eval_with(source: &str, bindings: &[(&str, Value)], limits: Limits) -> Result<Value, Error> {
    let mut alloc = HeapAllocator;
    eval_with_allocator(source, bindings, limits, &mut alloc)
}

/// 呼び出し側が指定した `Allocator` を用いて Rin ソースを評価する。
///
/// 読み込み段階の環境で 1 回評価する（`def`・`defn` 等も書ける）。
pub fn eval_with_allocator(
    source: &str,
    bindings: &[(&str, Value)],
    limits: Limits,
    alloc: &mut dyn Allocator,
) -> Result<Value, Error> {
    let mut builder = Builder::new(limits, alloc);
    for (k, v) in bindings {
        builder.bind(k, v)?;
    }
    builder.eval(source)
}

/// 読み込み段階の Rin 環境。
///
/// `bind`・`load_module`・`eval` で大域環境とモジュールを構築し、
/// [`Builder::freeze`] で実行専用の [`Machine`] へ移行する。
pub struct Builder<'a> {
    inner: EnvState<'a>,
}

/// 凍結された実行段階の Rin 環境。
///
/// 大域環境と読み込み済みモジュールを変更せず、各評価は互いに独立して実行される。
/// 読み込み済みのすべてのモジュールは `import` なしで `mod_name.symbol` として参照できる。
pub struct Machine<'a> {
    inner: EnvState<'a>,
}

/// 永続領域 (`PermArena`) 上に配置する環境スロット。
///
/// 組み込み名 (`id < STATIC_SYM_COUNT`) の上書きがない限り、ユーザー定義変数は 0 始まりの
/// 密な配列に格納されるため、モジュールや環境ごとに 54 スロット分の空き領域を消費しない。
#[derive(Clone, Copy)]
pub(crate) struct EnvSlots {
    dynamic: RawVec<Option<RVal>>,
    overrides: RawVec<(u32, RVal)>,
}

impl EnvSlots {
    pub(crate) const fn new() -> Self {
        Self {
            dynamic: RawVec::new(),
            overrides: RawVec::new(),
        }
    }

    #[inline]
    pub(crate) fn get(&self, id: u32) -> Option<RVal> {
        if id >= STATIC_SYM_COUNT {
            self.dynamic
                .as_slice()
                .get((id - STATIC_SYM_COUNT) as usize)
                .copied()
                .flatten()
        } else {
            for &(k, v) in self.overrides.as_slice() {
                if k == id {
                    return Some(v);
                }
            }
            None
        }
    }

    pub(crate) fn set(
        &mut self,
        pool: &mut Pool<'_>,
        id: u32,
        val: RVal,
        span: Span,
    ) -> Result<Option<RVal>, ErrRecorded> {
        if id >= STATIC_SYM_COUNT {
            set_slot_perm(&mut self.dynamic, pool, id - STATIC_SYM_COUNT, val, span)
        } else {
            for (k, v) in self.overrides.as_mut_slice() {
                if *k == id {
                    return Ok(Some(std::mem::replace(v, val)));
                }
            }
            if self
                .overrides
                .push_perm(&mut pool.perm, &mut pool.core, (id, val))
                .is_none()
            {
                return Err(pool.core.err_str(
                    "eval.limits",
                    ErrorKind::TooMuchMemory,
                    span,
                    "memory limit exceeded",
                ));
            }
            Ok(None)
        }
    }

    pub(crate) fn clear(&mut self, pool: &mut Pool<'_>) {
        for slot in self.dynamic.as_mut_slice() {
            if let Some(v) = slot.take() {
                pool.release_val(v);
            }
        }
        for &(_, v) in self.overrides.as_slice() {
            pool.release_val(v);
        }
        self.overrides.clear();
    }

    pub(crate) fn iter_defined(&self) -> impl Iterator<Item = (u32, RVal)> + '_ {
        let ov = self.overrides.as_slice().iter().copied();
        let dy = self
            .dynamic
            .as_slice()
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.map(|val| (STATIC_SYM_COUNT + i as u32, val)));
        ov.chain(dy)
    }
}

/// 永続領域 (`PermArena`) 上に配置する読み込み済みモジュール表。
pub(super) struct ModuleTable {
    ids: RawMap<RawStr, u32>,
    names: RawVec<RawStr>,
    vals: RawVec<Option<RVal>>,
    envs: RawVec<EnvSlots>,
}

impl ModuleTable {
    pub(super) const fn new() -> Self {
        Self {
            ids: RawMap::new(),
            names: RawVec::new(),
            vals: RawVec::new(),
            envs: RawVec::new(),
        }
    }

    pub(super) fn get_id(&self, name: &str) -> Option<u32> {
        let hash = fnv1a_bytes(name.as_bytes());
        self.ids.get_by(hash, |k| k.as_str() == name)
    }

    pub(super) fn get_val(&self, name: &str) -> Option<RVal> {
        let id = self.get_id(name)?;
        self.vals.as_slice()[id as usize]
    }

    pub(super) fn name(&self, id: u32) -> &str {
        self.names.as_slice()[id as usize].as_str()
    }

    pub(super) fn loaded_names(&self) -> impl Iterator<Item = &str> {
        let names = self.names.as_slice();
        let vals = self.vals.as_slice();
        (0..names.len()).filter_map(move |i| vals[i].map(|_| names[i].as_str()))
    }

    pub(super) fn ensure_module(
        &mut self,
        pool: &mut Pool<'_>,
        name: &str,
    ) -> Result<u32, ErrRecorded> {
        if let Some(id) = self.get_id(name) {
            return Ok(id);
        }
        let id = self.names.len() as u32;
        let Some(raw) = RawStr::from_perm(&mut pool.perm, &mut pool.core, name) else {
            return Err(pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            ));
        };
        if self
            .names
            .push_perm(&mut pool.perm, &mut pool.core, raw)
            .is_none()
        {
            return Err(pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            ));
        }
        if self
            .vals
            .push_perm(&mut pool.perm, &mut pool.core, None)
            .is_none()
        {
            self.names.pop();
            return Err(pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            ));
        }
        if self
            .envs
            .push_perm(&mut pool.perm, &mut pool.core, EnvSlots::new())
            .is_none()
        {
            self.vals.pop();
            self.names.pop();
            return Err(pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            ));
        }
        let hash = fnv1a_bytes(name.as_bytes());
        if self
            .ids
            .insert_perm(&mut pool.perm, &mut pool.core, hash, raw, id, |a, b| {
                a.as_str() == b.as_str()
            })
            .is_none()
        {
            self.envs.pop();
            self.vals.pop();
            self.names.pop();
            return Err(pool.core.err_str(
                "eval.limits",
                ErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            ));
        }
        Ok(id)
    }

    pub(super) fn env(&self, id: u32) -> &EnvSlots {
        &self.envs.as_slice()[id as usize]
    }

    pub(super) fn set_env_slot(
        &mut self,
        pool: &mut Pool<'_>,
        mod_id: u32,
        sym_id: u32,
        val: RVal,
        span: Span,
    ) -> Result<Option<RVal>, ErrRecorded> {
        self.envs.as_mut_slice()[mod_id as usize].set(pool, sym_id, val, span)
    }

    pub(super) fn clear_env(&mut self, id: u32, pool: &mut Pool<'_>) {
        self.envs.as_mut_slice()[id as usize].clear(pool);
    }

    pub(super) fn set_val(&mut self, id: u32, val: RVal) -> Option<RVal> {
        self.vals.as_mut_slice()[id as usize].replace(val)
    }

    pub(super) fn release_all(&mut self, pool: &mut Pool<'_>) {
        for slot in self.vals.as_mut_slice() {
            if let Some(v) = slot.take() {
                pool.release_val(v);
            }
        }
        for env in self.envs.as_mut_slice() {
            env.clear(pool);
        }
    }
}

pub(super) struct EnvState<'a> {
    limits: Limits,
    math_fma: MathFma,
    pool: Pool<'a>,
    syms: SymTable,
    globals: EnvSlots,
    modules: ModuleTable,
    last_result: RVal,
}

#[inline]
pub(crate) fn global_slot(globals: &EnvSlots, id: u32) -> Option<RVal> {
    if let Some(v) = globals.get(id) {
        return Some(v);
    }
    let idx = id as usize;
    if idx < BuiltinId::ALL.len() {
        Some(RVal::Builtin(BuiltinId::ALL[idx]))
    } else if id == PI_SYM_ID {
        Some(RVal::Float(std::f64::consts::PI))
    } else {
        None
    }
}

fn global_bound_names<'a>(
    syms: &'a SymTable,
    globals: &'a EnvSlots,
) -> impl Iterator<Item = &'a str> + 'a {
    let statics = (0..STATIC_SYM_COUNT).filter_map(|id| -> Option<&'a str> { static_sym_name(id) });
    let dynamics = globals
        .dynamic
        .as_slice()
        .iter()
        .enumerate()
        .filter_map(|(i, value)| value.and_then(|_| syms.name(STATIC_SYM_COUNT + i as u32)));
    statics.chain(dynamics)
}

fn env_bound_names<'a>(
    syms: &'a SymTable,
    env: &'a EnvSlots,
) -> impl Iterator<Item = &'a str> + 'a {
    env.iter_defined().filter_map(|(id, _)| syms.name(id))
}

fn set_slot_perm(
    slots: &mut RawVec<Option<RVal>>,
    pool: &mut Pool<'_>,
    id: u32,
    val: RVal,
    span: Span,
) -> Result<Option<RVal>, ErrRecorded> {
    let idx = id as usize;
    if idx >= slots.len()
        && slots
            .resize_perm(&mut pool.perm, &mut pool.core, idx + 1, None)
            .is_none()
    {
        return Err(pool.core.err_str(
            "eval.limits",
            ErrorKind::TooMuchMemory,
            span,
            "memory limit exceeded",
        ));
    }
    Ok(slots.as_mut_slice()[idx].replace(val))
}

#[derive(Clone, Copy)]
pub(super) enum CallOrigin {
    Rin(Span),
    Host,
    Builtin { name: &'static str, at: Span },
}

pub(super) const INLINE_EVAL_BINDINGS: usize = 8;

pub(super) enum EvalBindingValues {
    Inline {
        entries: [(u32, RVal); INLINE_EVAL_BINDINGS],
        len: usize,
    },
    Map(RawMap<u32, RVal>),
}

impl EvalBindingValues {
    pub(super) fn new(capacity_hint: usize) -> Self {
        if capacity_hint <= INLINE_EVAL_BINDINGS {
            Self::Inline {
                entries: [(0, RVal::Void); INLINE_EVAL_BINDINGS],
                len: 0,
            }
        } else {
            Self::Map(RawMap::new())
        }
    }

    pub(super) fn insert(
        &mut self,
        pool: &mut Pool<'_>,
        id: u32,
        val: RVal,
    ) -> Result<Option<RVal>, ErrRecorded> {
        match self {
            Self::Inline { entries, len } => {
                for entry in &mut entries[..*len] {
                    if entry.0 == id {
                        return Ok(Some(std::mem::replace(&mut entry.1, val)));
                    }
                }
                entries[*len] = (id, val);
                *len += 1;
                Ok(None)
            }
            Self::Map(map) => map
                .insert_work(
                    &mut pool.work,
                    &mut pool.core,
                    fnv1a_u32(id),
                    id,
                    val,
                    |a, b| a == b,
                )
                .ok_or_else(|| {
                    pool.core.err_str(
                        "eval.limits",
                        ErrorKind::TooMuchMemory,
                        Span::default(),
                        "memory limit exceeded",
                    )
                }),
        }
    }

    pub(super) fn get(&self, id: u32) -> Option<RVal> {
        match self {
            Self::Inline { entries, len } => {
                for &(k, v) in &entries[..*len] {
                    if k == id {
                        return Some(v);
                    }
                }
                None
            }
            Self::Map(map) => map.get_by(fnv1a_u32(id), |k| *k == id),
        }
    }

    pub(super) fn release_all(self, pool: &mut Pool<'_>) {
        match self {
            Self::Inline { entries, len } => {
                for &(_, v) in &entries[..len] {
                    pool.release_val(v);
                }
            }
            Self::Map(map) => {
                for (_, v) in map.iter() {
                    pool.release_val(v);
                }
            }
        }
    }
}

pub(super) enum EvalBindingNames<'a> {
    Values(&'a [(&'a str, Value)]),
    Vals(&'a [(&'a str, Val)]),
}

pub(super) struct EvalBindings<'a> {
    pub(super) values: EvalBindingValues,
    pub(super) names: EvalBindingNames<'a>,
}

pub(super) const INLINE_VALUE_STACK: usize = 16;

/// 浅い評価では作業領域を確保せず、16 要素を超えたときだけ `WorkArena` へ移す値スタック。
pub(super) struct ValueStack {
    inline: [RVal; INLINE_VALUE_STACK],
    overflow: crate::mem::RawVec<RVal>,
    len: usize,
    spilled: bool,
}

impl ValueStack {
    pub(super) const fn new() -> Self {
        Self {
            inline: [RVal::Void; INLINE_VALUE_STACK],
            overflow: crate::mem::RawVec::new(),
            len: 0,
            spilled: false,
        }
    }

    #[inline(always)]
    pub(super) fn len(&self) -> usize {
        if self.spilled {
            self.overflow.len()
        } else {
            self.len
        }
    }

    #[inline(always)]
    pub(super) fn as_slice(&self) -> &[RVal] {
        if self.spilled {
            self.overflow.as_slice()
        } else {
            &self.inline[..self.len]
        }
    }

    #[inline(always)]
    pub(super) fn as_mut_slice(&mut self) -> &mut [RVal] {
        if self.spilled {
            self.overflow.as_mut_slice()
        } else {
            &mut self.inline[..self.len]
        }
    }

    #[inline(always)]
    pub(super) fn last(&self) -> Option<&RVal> {
        self.as_slice().last()
    }

    #[inline(always)]
    pub(super) fn pop(&mut self) -> Option<RVal> {
        if self.spilled {
            self.overflow.pop()
        } else if self.len == 0 {
            None
        } else {
            self.len -= 1;
            Some(self.inline[self.len])
        }
    }

    #[inline(always)]
    pub(super) fn truncate(&mut self, new_len: usize) {
        if self.spilled {
            self.overflow.truncate(new_len);
        } else if new_len < self.len {
            self.len = new_len;
        }
    }

    fn spill(
        &mut self,
        work: &mut crate::mem::WorkArena,
        core: &mut crate::mem::AllocCore<'_>,
        additional: usize,
    ) -> Option<()> {
        debug_assert!(!self.spilled);
        let needed = self.len.checked_add(additional)?;
        self.overflow.reserve_work(work, core, needed)?;
        self.overflow
            .extend_from_slice_work(work, core, &self.inline[..self.len])?;
        self.spilled = true;
        Some(())
    }

    #[inline(always)]
    pub(super) fn push_work(
        &mut self,
        work: &mut crate::mem::WorkArena,
        core: &mut crate::mem::AllocCore<'_>,
        val: RVal,
    ) -> Option<()> {
        if !self.spilled {
            if self.len < INLINE_VALUE_STACK {
                self.inline[self.len] = val;
                self.len += 1;
                return Some(());
            }
            self.spill(work, core, 1)?;
        }
        self.overflow.push_work(work, core, val)
    }

    #[inline(always)]
    pub(super) fn reserve_work(
        &mut self,
        work: &mut crate::mem::WorkArena,
        core: &mut crate::mem::AllocCore<'_>,
        additional: usize,
    ) -> Option<()> {
        if !self.spilled {
            if self.len.saturating_add(additional) <= INLINE_VALUE_STACK {
                return Some(());
            }
            return self.spill(work, core, additional);
        }
        self.overflow.reserve_work(work, core, additional)
    }

    #[inline(always)]
    pub(super) fn resize_work(
        &mut self,
        work: &mut crate::mem::WorkArena,
        core: &mut crate::mem::AllocCore<'_>,
        new_len: usize,
        fill: RVal,
    ) -> Option<()> {
        if !self.spilled {
            if new_len <= self.len {
                self.len = new_len;
                return Some(());
            }
            if new_len <= INLINE_VALUE_STACK {
                for slot in &mut self.inline[self.len..new_len] {
                    *slot = fill;
                }
                self.len = new_len;
                return Some(());
            }
            self.spill(work, core, new_len - self.len)?;
        }
        self.overflow.resize_work(work, core, new_len, fill)
    }
}

pub(super) struct Runner<'a, 'p> {
    pub(super) limits: Limits,
    pub(super) math_fma: MathFma,
    pub(super) steps_left: usize,
    pub(super) depth: usize,
    pub(super) frozen: bool,
    pub(super) eval_bindings: Option<&'a EvalBindings<'a>>,
    pub(super) pool: &'a mut Pool<'p>,
    pub(super) syms: &'a SymTable,
    pub(super) globals: &'a mut EnvSlots,
    pub(super) modules: &'a mut ModuleTable,
    pub(super) active_module: Option<u32>,
    pub(super) export_list: Option<RawVec<RawStr>>,
    pub(super) stack: ValueStack,
    pub(super) check_failures: Vec<crate::test::CheckFailure>,
}

pub(super) enum FrameSource<'a> {
    Top(&'a Chunk),
    Proto(FnProtoPtr),
}

impl FrameSource<'_> {
    #[inline(always)]
    pub(super) fn chunk(&self) -> &Chunk {
        match self {
            FrameSource::Top(c) => c,
            FrameSource::Proto(p) => &p.chunk,
        }
    }
}
