//! C 言語から Rin を利用するためのゼロヒープ FFI 境界とマシン操作 API を実装する。

use std::alloc::Layout;
use std::ffi::{c_char, c_void};
use std::ptr::{NonNull, addr_of_mut};

use crate::alloc::Allocator;
use crate::error::{Limits, Span};
use crate::eval::Builder;
use crate::math::MathFma;
use crate::mem::{MemoryConfig, RegionConfig};
use crate::value_ref::Val;

mod error;
mod eval;
mod handles;
#[cfg(feature = "std-value")]
mod schema;
mod value;
mod value_read;

use error::StoredError;
pub use error::{RinError, RinErrorKind};
pub use eval::*;
pub use handles::RinValue;
use handles::{CAllocator, HandleSlot, RinState, ValueHandles};
#[cfg(feature = "std-value")]
pub use schema::*;
pub use value::*;
pub use value_read::*;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RinTag {
    Void = 0,
    Int = 1,
    Float = 2,
    Bool = 3,
    Char = 4,
    Text = 5,
    List = 6,
    Array = 7,
    Tuple = 8,
    Struct = 9,
    Variant = 10,
    None = 11,
    Some = 12,
    Ok = 13,
    Err = 14,
    Fn = 15,
    Ident = 16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RinMathFma {
    Strict = 0,
    Fused = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RinLimits {
    pub steps: usize,
    pub depth: usize,
    pub memory_bytes: usize,
}

impl From<RinLimits> for Limits {
    fn from(l: RinLimits) -> Self {
        Self {
            steps: l.steps,
            depth: l.depth,
            memory_bytes: l.memory_bytes,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RinRegionConfig {
    pub initial_block: usize,
    pub max_block: usize,
}

impl From<RinRegionConfig> for RegionConfig {
    fn from(c: RinRegionConfig) -> Self {
        Self {
            initial_block: c.initial_block,
            max_block: c.max_block,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RinMemoryConfig {
    pub perm: RinRegionConfig,
    pub obj: RinRegionConfig,
    pub work: RinRegionConfig,
}

impl From<RinMemoryConfig> for MemoryConfig {
    fn from(c: RinMemoryConfig) -> Self {
        Self {
            perm: c.perm.into(),
            obj: c.obj.into(),
            work: c.work.into(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RinAllocator {
    pub ctx: *mut c_void,
    pub alloc: Option<unsafe extern "C" fn(ctx: *mut c_void, size: usize, align: usize) -> *mut u8>,
    pub dealloc:
        Option<unsafe extern "C" fn(ctx: *mut c_void, ptr: *mut u8, size: usize, align: usize)>,
}

/// C API の Rin マシン本体。`RinAllocator` 上に直接確保される（システムヒープ不使用）。
pub struct Rin {
    state: Option<RinState>,
    handles: ValueHandles,
    last_result: HandleSlot,
    last_error: StoredError,
    has_error: bool,
    alloc: CAllocator,
}

impl Rin {
    fn state_ref(&self) -> &RinState {
        self.state.as_ref().expect("Rin state present")
    }

    fn state_mut(&mut self) -> &mut RinState {
        self.state.as_mut().expect("Rin state present")
    }

    fn clear_error(&mut self) {
        self.has_error = false;
    }

    fn record_host_error(&mut self, message: &str) {
        self.last_error.set_host(message);
        self.has_error = true;
    }

    fn record_simple_error(&mut self, rule: &str, kind: RinErrorKind, at: Span, message: &str) {
        self.last_error.set_simple(rule, kind, at, message);
        self.has_error = true;
    }

    fn record_simple_error_fmt(
        &mut self,
        rule: &str,
        kind: RinErrorKind,
        at: Span,
        args: std::fmt::Arguments<'_>,
    ) {
        self.last_error.set_simple_fmt(rule, kind, at, args);
        self.has_error = true;
    }

    fn alloc_handle(&mut self, val: Val) -> *mut RinValue {
        let mut alloc = self.alloc;
        match self.handles.alloc_slot(val, &mut alloc) {
            Some(ptr) => ptr,
            None => {
                self.state_mut().release_val(val);
                self.record_simple_error(
                    "eval.limits",
                    RinErrorKind::TooMuchMemory,
                    Span::default(),
                    "allocator exhausted while creating value handle",
                );
                std::ptr::null_mut()
            }
        }
    }

    fn free_handle(&mut self, ptr: *mut RinValue) {
        if ptr.is_null() {
            return;
        }
        // SAFETY: `ptr` は `alloc_handle` が返した有効なハンドル。
        if let Some(val) = unsafe { self.handles.free_slot(ptr) } {
            self.state_mut().release_val(val);
        }
    }

    unsafe fn owns_handle(&self, ptr: *const RinValue) -> bool {
        // SAFETY: 呼び出し側が有効なハンドルポインタを保証する。
        unsafe { self.handles.owns_live_val(ptr) }
    }

    fn finish_eval_result(&mut self) -> RinEvalResult {
        let val = self.state_ref().last_result_val();
        self.last_result.val = RinValue(val);
        RinEvalResult {
            ok: true,
            value: &self.last_result.val as *const RinValue,
        }
    }
}

// machine を確認してから、ほかの引数を検査する前に前回のエラーを消す。
unsafe fn begin_operation<'a>(machine: *mut Rin) -> Option<&'a mut Rin> {
    // SAFETY: 呼び出し側が NULL または排他的に利用できる有効な machine を保証する。
    let m = unsafe { machine.as_mut() }?;
    m.clear_error();
    Some(m)
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RinBinding {
    pub name: *const c_char,
    pub value: *const RinValue,
}

#[repr(C)]
pub struct RinEvalResult {
    pub ok: bool,
    pub value: *const RinValue,
}

impl RinEvalResult {
    pub(super) fn failure() -> Self {
        Self {
            ok: false,
            value: std::ptr::null(),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rin_limits_default() -> RinLimits {
    let d = Limits::default();
    RinLimits {
        steps: d.steps,
        depth: d.depth,
        memory_bytes: d.memory_bytes,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rin_memory_config_default() -> RinMemoryConfig {
    let d = MemoryConfig::default();
    RinMemoryConfig {
        perm: RinRegionConfig {
            initial_block: d.perm.initial_block,
            max_block: d.perm.max_block,
        },
        obj: RinRegionConfig {
            initial_block: d.obj.initial_block,
            max_block: d.obj.max_block,
        },
        work: RinRegionConfig {
            initial_block: d.work.initial_block,
            max_block: d.work.max_block,
        },
    }
}

/// 指定した `RinAllocator` 上に `Rin` 本体を直接確保して返す（システムヒープ不使用）。
/// 確保に失敗した場合は `NULL` を返す。
#[unsafe(no_mangle)]
pub extern "C" fn rin_new(limits: RinLimits, alloc: RinAllocator) -> *mut Rin {
    rin_new_with_config(limits, rin_memory_config_default(), alloc)
}

/// 3 領域のブロック設定 (`RinMemoryConfig`) を指定して `Rin` 本体を確保する（システムヒープ不使用）。
/// 確保に失敗した場合は `NULL` を返す。
#[unsafe(no_mangle)]
pub extern "C" fn rin_new_with_config(
    limits: RinLimits,
    memory: RinMemoryConfig,
    alloc: RinAllocator,
) -> *mut Rin {
    let mut calloc = CAllocator { raw: alloc };
    let layout = Layout::new::<Rin>();
    let Some(raw) = calloc.alloc(layout) else {
        return std::ptr::null_mut();
    };
    let ptr = raw.cast::<Rin>().as_ptr();
    // SAFETY: `raw` は `Layout::new::<Rin>()` で確保された未初期化領域。
    // 各フィールドを直接初期化し、`alloc` を指す参照は `rin_free` まで移動しない。
    unsafe {
        addr_of_mut!((*ptr).alloc).write(calloc);
        let alloc_ref: &'static mut dyn Allocator =
            &mut *(addr_of_mut!((*ptr).alloc) as *mut dyn Allocator);
        let builder = Builder::with_memory_config(limits.into(), memory.into(), alloc_ref);
        addr_of_mut!((*ptr).state).write(Some(RinState::Building(builder)));
        addr_of_mut!((*ptr).handles).write(ValueHandles::new());
        addr_of_mut!((*ptr).last_result).write(HandleSlot::empty());
        addr_of_mut!((*ptr).last_error).write(StoredError::new_empty());
        addr_of_mut!((*ptr).has_error).write(false);
    }
    ptr
}

/// # Safety
/// `machine` は `NULL` または `rin_new` が返したポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_free(machine: *mut Rin) {
    if machine.is_null() {
        return;
    }
    // SAFETY: `machine` は `rin_new` が `CAllocator` から確保した有効なポインタ。
    unsafe {
        let m = &mut *machine;
        let mut calloc = m.alloc;
        if let Some(state) = m.state.as_mut() {
            m.handles.release_all_and_destroy(state, &mut calloc);
        }
        drop(m.state.take());
        calloc.dealloc(
            NonNull::new_unchecked(machine.cast::<u8>()),
            Layout::new::<Rin>(),
        );
    }
}

/// 大域環境と読み込み済みモジュールを凍結し、実行の段階へ移行する。
/// すでに凍結済みの場合はそのまま `true` を返す。
///
/// # Safety
/// `machine` は NULL または排他的に利用できる有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_freeze(machine: *mut Rin) -> bool {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return false;
    };
    match m.state.take().expect("Rin state present") {
        RinState::Building(builder) => {
            let frozen = builder.into_machine();
            m.state = Some(RinState::Frozen(frozen));
            true
        }
        RinState::Frozen(frozen) => {
            m.state = Some(RinState::Frozen(frozen));
            true
        }
    }
}

/// 組み込み数学関数の FMA 設定を変更する。
///
/// # Safety
/// `machine` は NULL または排他的に利用できる有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_set_math_fma(machine: *mut Rin, mode: i32) -> bool {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return false;
    };
    let parsed = match mode {
        0 => MathFma::Strict,
        1 => MathFma::Fused,
        _ => {
            m.record_host_error("invalid math fma mode");
            return false;
        }
    };
    match m.state_mut() {
        RinState::Building(b) => b.set_math_fma(parsed),
        RinState::Frozen(f) => f.set_math_fma(parsed),
    }
    true
}

/// 現在の組み込み数学関数の FMA 設定を返す。`machine` が NULL なら `Strict` を返す。
///
/// # Safety
/// `machine` は NULL または有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_math_fma(machine: *const Rin) -> RinMathFma {
    // SAFETY: 呼び出し側が NULL または有効なポインタを保証する。
    let Some(m) = (unsafe { machine.as_ref() }) else {
        return RinMathFma::Strict;
    };
    let mode = match m.state_ref() {
        RinState::Building(b) => b.math_fma(),
        RinState::Frozen(f) => f.math_fma(),
    };
    match mode {
        MathFma::Strict => RinMathFma::Strict,
        MathFma::Fused => RinMathFma::Fused,
    }
}
