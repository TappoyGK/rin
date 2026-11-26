//! バイトコード VM の実行環境 `Builder`・`Machine` と `Runner` を定義する。

mod builtins;
mod host_vals;
mod machine;
mod modules;
mod ops;
mod runner;
mod vm;

use crate::alloc::Allocator;
#[cfg(feature = "std-value")]
use crate::alloc::HeapAllocator;
use crate::bytecode::{
    BuiltinId, Capture, Chunk, FnProtoPtr, PI_SYM_ID, STATIC_SYM_COUNT, SymTable, static_sym_name,
};
use crate::compiler::compile_program;
#[cfg(feature = "std-value")]
use crate::compiler::compile_program_parts;
use crate::diagnostic::{write_available, write_suggest};
#[cfg(feature = "std-value")]
use crate::error::Error;
use crate::error::{ErrRecorded, ErrorKind, ErrorRef, Limits, Span};
use crate::math::MathFma;
use crate::mem::{
    AllocCore, MemoryConfig, RawMap, RawStr, RawVec, WorkArena, fnv1a_bytes, fnv1a_u32,
};
use crate::parser::parse_program_work;
use crate::pool::{Obj, Pool, RVal};
#[cfg(feature = "std-value")]
use crate::value::Value;
use crate::value_ref::{Val, ValueRef};

/// 既定の `HeapAllocator` を用いて Rin ソースを評価する。
///
/// 読み込み段階の環境で 1 回評価する（`def`・`defn` 等も書ける）。
#[cfg(feature = "std-value")]
pub fn eval_with(source: &str, bindings: &[(&str, Value)], limits: Limits) -> Result<Value, Error> {
    let mut alloc = HeapAllocator;
    eval_with_allocator(source, bindings, limits, &mut alloc)
}

/// 呼び出し側が指定した `Allocator` を用いて Rin ソースを評価する。
///
/// 読み込み段階の環境で 1 回評価する（`def`・`defn` 等も書ける）。
#[cfg(feature = "std-value")]
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

    #[inline(always)]
    pub(crate) fn get(&self, id: u32) -> Option<RVal> {
        if id >= STATIC_SYM_COUNT {
            let idx = (id - STATIC_SYM_COUNT) as usize;
            if idx < self.dynamic.len() {
                self.dynamic.get_copy(idx)
            } else {
                None
            }
        } else {
            let n = self.overrides.len();
            for i in 0..n {
                let (k, v) = self.overrides.get_copy(i);
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

#[inline(always)]
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
    Builtin { id: BuiltinId, at: Span },
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
    #[cfg(feature = "std-value")]
    Values(&'a [(&'a str, Value)]),
    Vals(&'a [(&'a str, Val)]),
}

pub(super) struct EvalBindings<'a> {
    pub(super) values: EvalBindingValues,
    pub(super) names: EvalBindingNames<'a>,
}

/// 作業領域 (`WorkArena`) 上の値スタック（`if spilled` 分岐を無くしホットパスを最速化する）。
pub(super) type ValueStack = crate::mem::RawVec<RVal>;

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
    #[cfg(feature = "std-value")]
    pub(super) check_failures: Vec<crate::test::CheckFailure>,
}

/// 構造体フィールド `(name, val)` を `WorkArena` 上の安定マージソートで O(F log F) 整列する。
/// 同名フィールドの相対順序（後ろ勝ち）を保つ。
pub(super) fn sort_field_pairs_work<'s>(
    slice: &mut [(&'s str, RVal)],
    work: &mut WorkArena,
    core: &mut AllocCore<'_>,
) -> Option<()> {
    const MIN_RUN: usize = 16;
    let n = slice.len();
    if n < 2 {
        return Some(());
    }
    if n <= MIN_RUN {
        for cur in 1..n {
            let item = slice[cur];
            let mut lo = 0;
            let mut hi = cur;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if slice[mid].0 > item.0 {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            if lo < cur {
                slice.copy_within(lo..cur, lo + 1);
                slice[lo] = item;
            }
        }
        return Some(());
    }
    let mark = work.mark(core);
    let mut scratch = RawVec::<(&'s str, RVal)>::new();
    scratch.resize_work(work, core, n, ("", RVal::Void))?;
    let buf = scratch.as_mut_slice();

    // MIN_RUN ごとに安定二分挿入ソート
    let mut start = 0;
    while start < n {
        let end = (start + MIN_RUN).min(n);
        for cur in (start + 1)..end {
            let item = slice[cur];
            let mut lo = start;
            let mut hi = cur;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if slice[mid].0 > item.0 {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            if lo < cur {
                slice.copy_within(lo..cur, lo + 1);
                slice[lo] = item;
            }
        }
        start = end;
    }

    // ピンポン安定マージ
    let mut width = MIN_RUN;
    let mut from_slice = true;
    while width < n {
        let (src, dst): (&[_], &mut [_]) = if from_slice {
            (&*slice, &mut *buf)
        } else {
            (&*buf, &mut *slice)
        };
        let mut left = 0;
        while left < n {
            let mid = (left + width).min(n);
            let right = (left + 2 * width).min(n);
            if mid >= right || src[mid - 1].0 <= src[mid].0 {
                dst[left..right].copy_from_slice(&src[left..right]);
            } else {
                let mut i = left;
                let mut j = mid;
                let mut k = left;
                while i < mid && j < right {
                    if src[i].0 <= src[j].0 {
                        dst[k] = src[i];
                        i += 1;
                    } else {
                        dst[k] = src[j];
                        j += 1;
                    }
                    k += 1;
                }
                if i < mid {
                    let rem = mid - i;
                    dst[k..k + rem].copy_from_slice(&src[i..mid]);
                    k += rem;
                }
                if j < right {
                    let rem = right - j;
                    dst[k..k + rem].copy_from_slice(&src[j..right]);
                }
            }
            left += 2 * width;
        }
        width *= 2;
        from_slice = !from_slice;
    }
    if !from_slice {
        slice.copy_from_slice(buf);
    }
    work.rewind(core, mark);
    Some(())
}

#[derive(Clone, Copy)]
pub(super) enum FrameSource<'a> {
    Top(&'a Chunk),
    Proto(FnProtoPtr),
}
