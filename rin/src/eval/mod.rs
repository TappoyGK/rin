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
use crate::parser::{ParseScratch, ParseScratchGuard};
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
    frozen_modules: Vec<Option<RVal>>,
    module_ids: BTreeMap<String, u32>,
    module_names: Vec<String>,
    module_envs: Vec<Vec<Option<RVal>>>,
    parse_scratch: ParseScratch,
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

pub(super) struct EvalBindings<'a> {
    pub(super) entries: &'a [(u32, RVal)],
    pub(super) raw: &'a [(&'a str, Value)],
}

pub(super) struct Runner<'a, 'p> {
    pub(super) limits: Limits,
    pub(super) math_fma: MathFma,
    pub(super) steps_left: usize,
    pub(super) depth: usize,
    pub(super) pool: &'a mut Pool<'p>,
    pub(super) syms: &'a SymTable,
    pub(super) globals: &'a mut Vec<Option<RVal>>,
    pub(super) modules: &'a BTreeMap<String, RVal>,
    pub(super) frozen_modules: &'a [Option<RVal>],
    pub(super) module_names: &'a [String],
    pub(super) module_envs: &'a mut Vec<Vec<Option<RVal>>>,
    pub(super) active_module: Option<u32>,
    pub(super) export_list: Option<Vec<String>>,
    pub(super) stack: Vec<RVal>,
    pub(super) check_failures: Vec<crate::test::CheckFailure>,
    pub(super) frozen: bool,
    pub(super) eval_bindings: Option<&'a EvalBindings<'a>>,
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
