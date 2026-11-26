//! C 言語から Rin を利用するためのゼロヒープ FFI 境界とマシン操作 API を実装する。

use std::alloc::Layout;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::{NonNull, addr_of_mut};

use crate::alloc::{Allocator, HeapAllocator};
use crate::error::{Limits, Span};
use crate::eval::{Builder, Machine};
use crate::math::MathFma;
use crate::value_ref::{Val, ValueRef};

mod error;
mod schema;
mod value;

use error::StoredError;
pub use error::{RinError, RinErrorKind};
pub use schema::*;
pub use value::*;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RinTag {
    Void = 0,
    Int = 1,
    Float = 2,
    Bool = 3,
    Char = 4,
    Str = 5,
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
#[derive(Clone, Copy)]
pub struct RinAllocator {
    pub ctx: *mut c_void,
    pub alloc: Option<unsafe extern "C" fn(ctx: *mut c_void, size: usize, align: usize) -> *mut u8>,
    pub dealloc:
        Option<unsafe extern "C" fn(ctx: *mut c_void, ptr: *mut u8, size: usize, align: usize)>,
}

#[derive(Clone, Copy)]
pub(super) struct CAllocator {
    raw: RinAllocator,
}

impl Allocator for CAllocator {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        if let Some(f) = self.raw.alloc {
            // SAFETY: 呼び出し側がマシンの生存期間中 `f` と `self.raw.ctx` の有効性を保証する。
            let ptr = unsafe { f(self.raw.ctx, layout.size(), layout.align()) };
            NonNull::new(ptr)
        } else {
            HeapAllocator.alloc(layout)
        }
    }

    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        if let Some(f) = self.raw.dealloc {
            // SAFETY: 呼び出し側が `alloc` に対応する `f` であることを保証する。
            unsafe { f(self.raw.ctx, ptr.as_ptr(), layout.size(), layout.align()) };
        } else if self.raw.alloc.is_none() {
            // SAFETY: `self.raw.alloc` が `None` のため `HeapAllocator` で確保した領域。
            unsafe { HeapAllocator.dealloc(ptr, layout) };
        }
    }
}

/// マシン所有の値ハンドル。`rin_int` 等で取得し `rin_drop` で解放する。
#[repr(transparent)]
pub struct RinValue(pub(super) Val);

const INLINE_HANDLES: usize = 32;
const CHUNK_HANDLES: usize = 64;

#[repr(C)]
struct HandleSlot {
    val: RinValue,
    next_free: *mut HandleSlot,
    alive: bool,
}

impl HandleSlot {
    const fn empty() -> Self {
        Self {
            val: RinValue(Val::void()),
            next_free: std::ptr::null_mut(),
            alive: false,
        }
    }
}

struct HandleChunk {
    next: *mut HandleChunk,
    used: usize,
    slots: [HandleSlot; CHUNK_HANDLES],
}

/// `RinValue` ハンドルの安定ポインタを `CAllocator` のみで管理するプール。
struct ValueHandles {
    inline: [HandleSlot; INLINE_HANDLES],
    inline_used: usize,
    chunks_head: *mut HandleChunk,
    free_head: *mut HandleSlot,
}

impl ValueHandles {
    const fn new() -> Self {
        Self {
            inline: [const { HandleSlot::empty() }; INLINE_HANDLES],
            inline_used: 0,
            chunks_head: std::ptr::null_mut(),
            free_head: std::ptr::null_mut(),
        }
    }

    fn alloc_slot(&mut self, val: Val, alloc: &mut CAllocator) -> Option<*mut RinValue> {
        if !self.free_head.is_null() {
            let slot = self.free_head;
            // SAFETY: `free_head` は当プール所有の有効な空きスロットを指す。
            unsafe {
                self.free_head = (*slot).next_free;
                (*slot).val = RinValue(val);
                (*slot).next_free = std::ptr::null_mut();
                (*slot).alive = true;
                return Some(addr_of_mut!((*slot).val));
            }
        }
        if self.inline_used < INLINE_HANDLES {
            let slot = &mut self.inline[self.inline_used];
            self.inline_used += 1;
            slot.val = RinValue(val);
            slot.next_free = std::ptr::null_mut();
            slot.alive = true;
            return Some(addr_of_mut!(slot.val));
        }
        // SAFETY: `chunks_head` は NULL または `alloc` から確保した有効な `HandleChunk`。
        let need_new = self.chunks_head.is_null() || unsafe { (*self.chunks_head).used } == CHUNK_HANDLES;
        if need_new {
            let layout = Layout::new::<HandleChunk>();
            let raw = alloc.alloc(layout)?;
            let chunk = raw.cast::<HandleChunk>().as_ptr();
            // SAFETY: `raw` は `HandleChunk` のレイアウトで確保済み。
            unsafe {
                addr_of_mut!((*chunk).next).write(self.chunks_head);
                addr_of_mut!((*chunk).used).write(0);
                let slots_ptr = addr_of_mut!((*chunk).slots).cast::<HandleSlot>();
                for i in 0..CHUNK_HANDLES {
                    slots_ptr.add(i).write(HandleSlot::empty());
                }
            }
            self.chunks_head = chunk;
        }
        // SAFETY: `chunks_head` は非 NULL かつ `used < CHUNK_HANDLES`。
        unsafe {
            let chunk = &mut *self.chunks_head;
            let slot = &mut chunk.slots[chunk.used];
            chunk.used += 1;
            slot.val = RinValue(val);
            slot.next_free = std::ptr::null_mut();
            slot.alive = true;
            Some(addr_of_mut!(slot.val))
        }
    }

    /// `val_ptr` を解放リストへ返し、保持していた `Val` を返す。
    unsafe fn free_slot(&mut self, val_ptr: *mut RinValue) -> Option<Val> {
        let slot = val_ptr.cast::<HandleSlot>();
        // SAFETY: `val_ptr` は `alloc_slot` が返した `HandleSlot.val` の先頭アドレス。
        unsafe {
            if !(*slot).alive {
                return None;
            }
            let val = (*slot).val.0;
            (*slot).alive = false;
            (*slot).val = RinValue(Val::void());
            (*slot).next_free = self.free_head;
            self.free_head = slot;
            Some(val)
        }
    }

    fn release_all_and_destroy(&mut self, state: &mut RinState, alloc: &mut CAllocator) {
        for slot in &mut self.inline[..self.inline_used] {
            if slot.alive {
                slot.alive = false;
                state.release_val(slot.val.0);
            }
        }
        let mut cur = self.chunks_head;
        self.chunks_head = std::ptr::null_mut();
        self.free_head = std::ptr::null_mut();
        let layout = Layout::new::<HandleChunk>();
        while !cur.is_null() {
            // SAFETY: `cur` は `alloc.alloc(Layout::new::<HandleChunk>())` で確保したチャンク。
            unsafe {
                let next = (*cur).next;
                let used = (*cur).used;
                let slots_ptr = addr_of_mut!((*cur).slots).cast::<HandleSlot>();
                for i in 0..used {
                    let slot = &mut *slots_ptr.add(i);
                    if slot.alive {
                        slot.alive = false;
                        state.release_val(slot.val.0);
                    }
                }
                alloc.dealloc(NonNull::new_unchecked(cur.cast::<u8>()), layout);
                cur = next;
            }
        }
    }
}

pub(super) enum RinState {
    Building(Builder<'static>),
    Frozen(Machine<'static>),
}

impl RinState {
    pub(super) fn release_val(&mut self, v: Val) {
        match self {
            RinState::Building(b) => b.release_val(v),
            RinState::Frozen(f) => f.release_val(v),
        }
    }

    pub(super) fn retain_val(&mut self, v: Val) {
        match self {
            RinState::Building(b) => b.retain_val(v),
            RinState::Frozen(f) => f.retain_val(v),
        }
    }

    pub(super) fn value_ref(&self, v: Val) -> ValueRef<'_> {
        match self {
            RinState::Building(b) => b.value_ref(v),
            RinState::Frozen(f) => f.value_ref(v),
        }
    }

    pub(super) fn last_result_val(&self) -> Val {
        match self {
            RinState::Building(b) => b.last_result_val(),
            RinState::Frozen(f) => f.last_result_val(),
        }
    }
}

/// C API の Rin マシン本体。`RinAllocator` 上に直接確保される（システムヒープ不使用）。
pub struct Rin {
    state: Option<RinState>,
    handles: ValueHandles,
    last_result: RinValue,
    last_error: StoredError,
    has_error: bool,
    alloc: CAllocator,
}

impl Rin {
    pub(super) fn state_ref(&self) -> &RinState {
        self.state.as_ref().expect("Rin state present")
    }

    pub(super) fn state_mut(&mut self) -> &mut RinState {
        self.state.as_mut().expect("Rin state present")
    }

    pub(super) fn clear_error(&mut self) {
        self.has_error = false;
    }

    pub(super) fn record_host_error(&mut self, message: &str) {
        self.last_error.set_host(message);
        self.has_error = true;
    }

    pub(super) fn record_simple_error(
        &mut self,
        rule: &str,
        kind: RinErrorKind,
        at: Span,
        message: &str,
    ) {
        self.last_error.set_simple(rule, kind, at, message);
        self.has_error = true;
    }

    pub(super) fn record_simple_error_fmt(
        &mut self,
        rule: &str,
        kind: RinErrorKind,
        at: Span,
        args: std::fmt::Arguments<'_>,
    ) {
        self.last_error.set_simple_fmt(rule, kind, at, args);
        self.has_error = true;
    }

    pub(super) fn alloc_handle(&mut self, val: Val) -> *mut RinValue {
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

    pub(super) fn free_handle(&mut self, ptr: *mut RinValue) {
        if ptr.is_null() {
            return;
        }
        // SAFETY: `ptr` は `alloc_handle` が返した有効なハンドル。
        if let Some(val) = unsafe { self.handles.free_slot(ptr) } {
            self.state_mut().release_val(val);
        }
    }

    fn finish_eval_result(&mut self) -> RinEvalResult {
        let val = self.state_ref().last_result_val();
        self.last_result = RinValue(val);
        RinEvalResult {
            ok: true,
            value: &self.last_result as *const RinValue,
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
    fn failure() -> Self {
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

/// 指定した `RinAllocator` 上に `Rin` 本体を直接確保して返す（システムヒープ不使用）。
/// 確保に失敗した場合は `NULL` を返す。
#[unsafe(no_mangle)]
pub extern "C" fn rin_new(limits: RinLimits, alloc: RinAllocator) -> *mut Rin {
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
        let builder = Builder::new(limits.into(), alloc_ref);
        addr_of_mut!((*ptr).state).write(Some(RinState::Building(builder)));
        addr_of_mut!((*ptr).handles).write(ValueHandles::new());
        addr_of_mut!((*ptr).last_result).write(RinValue(Val::void()));
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
            let frozen = builder.freeze().expect("freeze does not fail");
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

/// # Safety
/// 各ポインタは NULL または有効でなければならない。非 NULL の `name` は NUL 終端。
/// machine は排他的に利用する。不正 UTF-8 と NULL 引数はエラーとして扱う。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_bind(
    machine: *mut Rin,
    name: *const c_char,
    val: *const RinValue,
) -> bool {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return false;
    };
    if name.is_null() || val.is_null() {
        m.record_host_error("null pointer argument");
        return false;
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(key) = unsafe { CStr::from_ptr(name) }.to_str() else {
        m.record_host_error("invalid UTF-8 binding name");
        return false;
    };
    // SAFETY: 呼び出し側が有効な `RinValue` を保証する。
    let v = unsafe { (*val).0 };
    let err_recorded = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => match b.bind_val(key, v) {
            Ok(()) => return true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                true
            }
        },
        RinState::Frozen(_) => {
            m.record_simple_error(
                "env.frozen",
                RinErrorKind::Runtime,
                Span::default(),
                "cannot bind in a frozen environment",
            );
            return false;
        }
    };
    if err_recorded {
        m.has_error = true;
    }
    false
}

/// 読み込み段階の環境でソースを評価する（`def`・`defn`・`import` 等を許可する）。
///
/// # Safety
/// 各ポインタは NULL または有効でなければならない。非 NULL の `source` は NUL 終端。
/// machine は排他的に利用する。不正 UTF-8 と NULL 引数はエラーとして扱う。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_build(machine: *mut Rin, source: *const c_char) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if source.is_null() {
        m.record_host_error("null pointer argument");
        return RinEvalResult::failure();
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(src) = unsafe { CStr::from_ptr(source) }.to_str() else {
        m.record_host_error("invalid UTF-8 source");
        return RinEvalResult::failure();
    };

    let ok = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => match b.eval_ref(src) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        RinState::Frozen(_) => {
            m.record_simple_error(
                "env.frozen",
                RinErrorKind::Runtime,
                Span::default(),
                "cannot call rin_build on a frozen Rin",
            );
            false
        }
    };
    if ok {
        m.finish_eval_result()
    } else {
        RinEvalResult::failure()
    }
}

/// 凍結された実行段階の環境でソースを評価する。
///
/// # Safety
/// 各ポインタは NULL または有効でなければならない。非 NULL の `source` は NUL 終端。
/// machine は排他的に利用する。不正 UTF-8 と NULL 引数はエラーとして扱う。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_eval(machine: *mut Rin, source: *const c_char) -> RinEvalResult {
    // SAFETY: 呼び出し側が引数の契約を満たしている。
    unsafe { eval_frozen_inner(machine, source, std::ptr::null(), 0, "rin_eval") }
}

/// 凍結された実行段階の環境で、この 1 回の評価にだけ有効な初期束縛を与えてソースを評価する。
///
/// # Safety
/// machine は NULL または排他的に利用できる有効なポインタとする。
/// source は NULL または NUL 終端文字列。bindings は NULL または binding_count 個の要素を持つ配列。
/// 各要素の name は NULL または NUL 終端文字列、value は NULL または有効な RinValue を指す。
/// 不正 UTF-8 と NULL 引数はエラーとして扱う。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_eval_with(
    machine: *mut Rin,
    source: *const c_char,
    bindings: *const RinBinding,
    binding_count: usize,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が引数の契約を満たしている。
    unsafe { eval_frozen_inner(machine, source, bindings, binding_count, "rin_eval_with") }
}

const STACK_FFI_ARGS: usize = 16;

unsafe fn eval_frozen_inner(
    machine: *mut Rin,
    source: *const c_char,
    bindings: *const RinBinding,
    binding_count: usize,
    fn_name: &str,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if source.is_null() || (binding_count > 0 && bindings.is_null()) {
        m.record_host_error("null pointer argument");
        return RinEvalResult::failure();
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(src) = unsafe { CStr::from_ptr(source) }.to_str() else {
        m.record_host_error("invalid UTF-8 source");
        return RinEvalResult::failure();
    };

    if matches!(m.state_ref(), RinState::Building(_)) {
        // 先に bindings の引数エラーを検査する。
        if binding_count > 0 {
            let raw_bindings = unsafe { std::slice::from_raw_parts(bindings, binding_count) };
            for b in raw_bindings {
                if b.name.is_null() || b.value.is_null() {
                    m.record_host_error("null pointer argument");
                    return RinEvalResult::failure();
                }
                if unsafe { CStr::from_ptr(b.name) }.to_str().is_err() {
                    m.record_host_error("invalid UTF-8 binding name");
                    return RinEvalResult::failure();
                }
            }
        }
        m.record_simple_error_fmt(
            "env.phase",
            RinErrorKind::Runtime,
            Span::default(),
            format_args!(
                "{fn_name} requires a frozen Rin; call rin_freeze first or use rin_build"
            ),
        );
        return RinEvalResult::failure();
    }

    let mut stack_buf: [(&str, Val); STACK_FFI_ARGS] = [("", Val::void()); STACK_FFI_ARGS];
    let mut heap_ptr: Option<(NonNull<(&str, Val)>, Layout)> = None;
    let slice_mut: &mut [(&str, Val)] = if binding_count <= STACK_FFI_ARGS {
        &mut stack_buf[..binding_count]
    } else {
        let Some(layout) = Layout::array::<(&str, Val)>(binding_count).ok() else {
            m.record_simple_error(
                "eval.limits",
                RinErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return RinEvalResult::failure();
        };
        let mut calloc = m.alloc;
        let Some(raw) = calloc.alloc(layout) else {
            m.record_simple_error(
                "eval.limits",
                RinErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return RinEvalResult::failure();
        };
        let p = raw.cast::<(&str, Val)>();
        heap_ptr = Some((p, layout));
        // SAFETY: `p` は `binding_count` 個分確保済み。すぐ下で全要素初期化する。
        unsafe {
            for i in 0..binding_count {
                p.as_ptr().add(i).write(("", Val::void()));
            }
            std::slice::from_raw_parts_mut(p.as_ptr(), binding_count)
        }
    };

    let mut filled = 0usize;
    if binding_count > 0 {
        // SAFETY: binding_count > 0 かつ bindings 非 NULL。
        let raw_bindings = unsafe { std::slice::from_raw_parts(bindings, binding_count) };
        for (i, b) in raw_bindings.iter().enumerate() {
            if b.name.is_null() || b.value.is_null() {
                for &(_, v) in &slice_mut[..filled] {
                    m.state_mut().release_val(v);
                }
                if let Some((p, layout)) = heap_ptr {
                    let mut calloc = m.alloc;
                    unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
                }
                m.record_host_error("null pointer argument");
                return RinEvalResult::failure();
            }
            let Ok(name) = unsafe { CStr::from_ptr(b.name) }.to_str() else {
                for &(_, v) in &slice_mut[..filled] {
                    m.state_mut().release_val(v);
                }
                if let Some((p, layout)) = heap_ptr {
                    let mut calloc = m.alloc;
                    unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
                }
                m.record_host_error("invalid UTF-8 binding name");
                return RinEvalResult::failure();
            };
            let v = unsafe { (*b.value).0 };
            // 前回の `last_result` をそのまま引数に渡しても安全にするため retain する。
            m.state_mut().retain_val(v);
            slice_mut[i] = (name, v);
            filled += 1;
        }
    }

    let ok = match m.state.as_mut().expect("Rin state present") {
        RinState::Frozen(f) => match f.eval_with_vals(src, slice_mut) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        RinState::Building(_) => unreachable!(),
    };

    for &(_, v) in &slice_mut[..filled] {
        m.state_mut().release_val(v);
    }
    if let Some((p, layout)) = heap_ptr {
        let mut calloc = m.alloc;
        unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
    }

    if ok {
        m.finish_eval_result()
    } else {
        RinEvalResult::failure()
    }
}

/// 定義済みの Rin 関数をソース再解析なしで呼び出す。
///
/// # Safety
/// machine は NULL または排他的に利用できる有効なポインタとする。
/// fn_name は NULL または NUL 終端文字列。args は NULL または argc 個の要素を持つ配列。
/// 各引数値は NULL または有効な RinValue を指す。不正 UTF-8 と NULL 引数はエラー。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_call(
    machine: *mut Rin,
    fn_name: *const c_char,
    args: *const *const RinValue,
    argc: usize,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if fn_name.is_null() || (argc > 0 && args.is_null()) {
        m.record_host_error("null pointer argument");
        return RinEvalResult::failure();
    }
    let Ok(name) = (unsafe { CStr::from_ptr(fn_name) }).to_str() else {
        m.record_host_error("invalid UTF-8 function name");
        return RinEvalResult::failure();
    };
    unsafe { call_with_args(m, None, name, args, argc) }
}

/// ホスト提供の UTF-8 ソースから名前付きモジュールを読み込む。
///
/// # Safety
/// machine は NULL または排他的に利用できる有効なポインタとする。
/// mod_name と source は NULL または有効な NUL 終端文字列。不正 UTF-8 と NULL 引数はエラー。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_load(
    machine: *mut Rin,
    mod_name: *const c_char,
    source: *const c_char,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if mod_name.is_null() || source.is_null() {
        m.record_host_error("null pointer argument");
        return RinEvalResult::failure();
    }
    let (Ok(name), Ok(src)) = (
        unsafe { CStr::from_ptr(mod_name) }.to_str(),
        unsafe { CStr::from_ptr(source) }.to_str(),
    ) else {
        m.record_host_error("invalid UTF-8 argument");
        return RinEvalResult::failure();
    };
    let ok = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => match b.load_module_ref(name, src) {
            Ok(()) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        RinState::Frozen(_) => {
            m.record_simple_error(
                "env.frozen",
                RinErrorKind::Runtime,
                Span::default(),
                "cannot load module in a frozen environment",
            );
            false
        }
    };
    if ok {
        RinEvalResult {
            ok: true,
            value: std::ptr::null(),
        }
    } else {
        RinEvalResult::failure()
    }
}

/// 読み込み済みモジュール `mod_name` 内の公開関数 `fn_name` を呼び出す。
///
/// # Safety
/// machine は NULL または排他的に利用できる有効なポインタとする。
/// mod_name と fn_name は NULL または NUL 終端文字列。args は NULL または argc 個の要素を持つ配列。
/// 各引数値は NULL または有効な RinValue を指す。不正 UTF-8 と NULL 引数はエラー。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_call_in(
    machine: *mut Rin,
    mod_name: *const c_char,
    fn_name: *const c_char,
    args: *const *const RinValue,
    argc: usize,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if mod_name.is_null() || fn_name.is_null() || (argc > 0 && args.is_null()) {
        m.record_host_error("null pointer argument");
        return RinEvalResult::failure();
    }
    let (Ok(mname), Ok(fname)) = (
        unsafe { CStr::from_ptr(mod_name) }.to_str(),
        unsafe { CStr::from_ptr(fn_name) }.to_str(),
    ) else {
        m.record_host_error("invalid UTF-8 argument");
        return RinEvalResult::failure();
    };
    unsafe { call_with_args(m, Some(mname), fname, args, argc) }
}

unsafe fn call_with_args(
    m: &mut Rin,
    mod_name: Option<&str>,
    fn_name: &str,
    args: *const *const RinValue,
    argc: usize,
) -> RinEvalResult {
    let mut stack_buf: [Val; STACK_FFI_ARGS] = [Val::void(); STACK_FFI_ARGS];
    let mut heap_ptr: Option<(NonNull<Val>, Layout)> = None;
    let slice_mut: &mut [Val] = if argc <= STACK_FFI_ARGS {
        &mut stack_buf[..argc]
    } else {
        let Some(layout) = Layout::array::<Val>(argc).ok() else {
            m.record_simple_error(
                "eval.limits",
                RinErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return RinEvalResult::failure();
        };
        let mut calloc = m.alloc;
        let Some(raw) = calloc.alloc(layout) else {
            m.record_simple_error(
                "eval.limits",
                RinErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return RinEvalResult::failure();
        };
        let p = raw.cast::<Val>();
        heap_ptr = Some((p, layout));
        unsafe {
            for i in 0..argc {
                p.as_ptr().add(i).write(Val::void());
            }
            std::slice::from_raw_parts_mut(p.as_ptr(), argc)
        }
    };

    let mut filled = 0usize;
    if argc > 0 {
        // SAFETY: 呼び出し側が `args` に `argc` 個の要素があることを保証する。
        let raw_args = unsafe { std::slice::from_raw_parts(args, argc) };
        for (i, &ptr) in raw_args.iter().enumerate() {
            if ptr.is_null() {
                for &v in &slice_mut[..filled] {
                    m.state_mut().release_val(v);
                }
                if let Some((p, layout)) = heap_ptr {
                    let mut calloc = m.alloc;
                    unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
                }
                m.record_host_error("null argument value");
                return RinEvalResult::failure();
            }
            let v = unsafe { (*ptr).0 };
            m.state_mut().retain_val(v);
            slice_mut[i] = v;
            filled += 1;
        }
    }

    let ok = match (m.state.as_mut().expect("Rin state present"), mod_name) {
        (RinState::Building(b), None) => match b.call_ref(fn_name, slice_mut) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        (RinState::Frozen(f), None) => match f.call_ref(fn_name, slice_mut) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        (RinState::Building(b), Some(mn)) => match b.call_in_ref(mn, fn_name, slice_mut) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        (RinState::Frozen(f), Some(mn)) => match f.call_in_ref(mn, fn_name, slice_mut) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
    };

    for &v in &slice_mut[..filled] {
        m.state_mut().release_val(v);
    }
    if let Some((p, layout)) = heap_ptr {
        let mut calloc = m.alloc;
        unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
    }

    if ok {
        m.finish_eval_result()
    } else {
        RinEvalResult::failure()
    }
}

/// 直近の対象操作のエラーを借用する。エラーなし、または NULL machine なら NULL。
/// 次の rin_build / rin_eval / rin_eval_with / rin_load / rin_call / rin_call_in / rin_bind / rin_freeze / rin_set_math_fma、または rin_free まで有効。
/// この取得関数はエラーを無効化しない。ホストは解放しない。
///
/// # Safety
/// `machine` は NULL または有効な Rin を指し、対象操作と同時に呼ばないこと。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_last_error(machine: *const Rin) -> *const RinError {
    // SAFETY: 呼び出し側が NULL または有効なポインタを保証する。
    let Some(m) = (unsafe { machine.as_ref() }) else {
        return std::ptr::null();
    };
    if m.has_error {
        &m.last_error.view
    } else {
        std::ptr::null()
    }
}
