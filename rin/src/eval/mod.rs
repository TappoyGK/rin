//! バイトコード VM の実行環境 `Builder`・`Machine` と `Runner` を定義する。

mod builtins;
mod machine;
mod modules;
mod ops;
mod runner;
mod vm;

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::alloc::{Allocator, HeapAllocator};
use crate::bytecode::{BuiltinId, Capture, Chunk, FnProto, SymTable};
use crate::compiler::{compile_program, compile_program_parts};
use crate::diagnostic::{available, suggest};
use crate::error::{Error, ErrorKind, Limits, Span};
use crate::math::MathFma;
use crate::parser::parse_program_work;
use crate::pool::{Obj, Pool, RVal};
use crate::value::Value;

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

pub(super) struct EnvState<'a> {
    limits: Limits,
    math_fma: MathFma,
    pool: Pool<'a>,
    syms: SymTable,
    globals: Vec<Option<RVal>>,
    modules: BTreeMap<String, RVal>,
    module_ids: BTreeMap<String, u32>,
    module_names: Vec<String>,
    module_envs: Vec<Vec<Option<RVal>>>,
}

fn bound_names<'a>(
    syms: &'a SymTable,
    values: &'a [Option<RVal>],
) -> impl Iterator<Item = &'a str> {
    values
        .iter()
        .enumerate()
        .filter_map(|(id, value)| value.map(|_| syms.names[id].as_ref()))
}

fn set_slot(slots: &mut Vec<Option<RVal>>, id: u32, val: RVal) -> Option<RVal> {
    let idx = id as usize;
    if idx >= slots.len() {
        slots.resize(idx + 1, None);
    }
    slots[idx].replace(val)
}

#[derive(Clone, Copy)]
pub(super) enum CallOrigin<'a> {
    Rin(Span),
    Host,
    Builtin { name: &'a str, at: Span },
}

pub(super) const INLINE_EVAL_BINDINGS: usize = 8;

pub(super) enum EvalBindingValues {
    Inline {
        entries: [(u32, RVal); INLINE_EVAL_BINDINGS],
        len: usize,
    },
    Map(BTreeMap<u32, RVal>),
}

impl EvalBindingValues {
    pub(super) fn new(capacity_hint: usize) -> Self {
        if capacity_hint <= INLINE_EVAL_BINDINGS {
            Self::Inline {
                entries: [(0, RVal::Void); INLINE_EVAL_BINDINGS],
                len: 0,
            }
        } else {
            Self::Map(BTreeMap::new())
        }
    }

    pub(super) fn insert(&mut self, id: u32, val: RVal) -> Option<RVal> {
        match self {
            Self::Inline { entries, len } => {
                for entry in &mut entries[..*len] {
                    if entry.0 == id {
                        return Some(std::mem::replace(&mut entry.1, val));
                    }
                }
                entries[*len] = (id, val);
                *len += 1;
                None
            }
            Self::Map(map) => map.insert(id, val),
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
            Self::Map(map) => map.get(&id).copied(),
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
                for v in map.into_values() {
                    pool.release_val(v);
                }
            }
        }
    }
}

pub(super) struct EvalBindings<'a> {
    pub(super) values: EvalBindingValues,
    pub(super) raw: &'a [(&'a str, Value)],
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
    pub(super) globals: &'a mut Vec<Option<RVal>>,
    pub(super) modules: &'a BTreeMap<String, RVal>,
    pub(super) module_names: &'a [String],
    pub(super) module_envs: &'a mut Vec<Vec<Option<RVal>>>,
    pub(super) active_module: Option<u32>,
    pub(super) export_list: Option<Vec<String>>,
    pub(super) stack: ValueStack,
    pub(super) check_failures: Vec<crate::test::CheckFailure>,
}

pub(super) enum FrameSource<'a> {
    Top(&'a Chunk),
    Proto(Rc<FnProto>),
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
