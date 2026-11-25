//! C 言語から Rin を利用するための FFI 境界とマシン操作 API を実装する。

use std::alloc::Layout;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;

use crate::alloc::{Allocator, HeapAllocator};
use crate::error::{Error, Limits, Span};
use crate::eval::{Builder, Machine};
use crate::math::MathFma;
use crate::value::Value;

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

struct CAllocator {
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

enum RinMachineState {
    Building(Builder<'static>),
    Frozen(Machine<'static>),
}

pub struct RinMachine {
    // `Pool::drop` が `_alloc` を呼ぶため、`_alloc` より先に `state` を破棄する。
    state: Option<RinMachineState>,
    last_error: Option<StoredError>,
    _alloc: Box<CAllocator>,
}

impl RinMachine {
    fn state_ref(&self) -> &RinMachineState {
        self.state.as_ref().expect("RinMachine state present")
    }

    fn state_mut(&mut self) -> &mut RinMachineState {
        self.state.as_mut().expect("RinMachine state present")
    }
}

fn make_err_res(m: &mut RinMachine, err: Error) -> RinEvalResult {
    m.last_error = Some(StoredError::from_error(err));
    RinEvalResult::failure()
}

fn host_error(m: &mut RinMachine, message: &str) -> RinEvalResult {
    m.last_error = Some(StoredError::host(message));
    RinEvalResult::failure()
}

// machine を確認してから、ほかの引数を検査する前に前回のエラーを消す。
unsafe fn begin_operation<'a>(machine: *mut RinMachine) -> Option<&'a mut RinMachine> {
    // SAFETY: 呼び出し側が NULL または排他的に利用できる有効な machine を保証する。
    let m = unsafe { machine.as_mut() }?;
    m.last_error = None;
    Some(m)
}

pub struct RinValue(pub Value);

#[repr(C)]
pub struct RinEvalResult {
    pub ok: bool,
    pub value: *mut RinValue,
}

impl RinEvalResult {
    fn failure() -> Self {
        Self {
            ok: false,
            value: std::ptr::null_mut(),
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
pub extern "C" fn rin_new(limits: RinLimits, alloc: RinAllocator) -> *mut RinMachine {
    let mut boxed_alloc = Box::new(CAllocator { raw: alloc });
    // SAFETY: `boxed_alloc` は `RinMachine` 内に保持し、`machine` より後に破棄する。
    let alloc_ref: &'static mut dyn Allocator =
        unsafe { &mut *(boxed_alloc.as_mut() as *mut CAllocator as *mut dyn Allocator) };
    let builder = Builder::new(limits.into(), alloc_ref);
    Box::into_raw(Box::new(RinMachine {
        state: Some(RinMachineState::Building(builder)),
        last_error: None,
        _alloc: boxed_alloc,
    }))
}

/// # Safety
/// `machine` は `NULL` または `rin_new` が返したポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_free(machine: *mut RinMachine) {
    if !machine.is_null() {
        // SAFETY: `machine` は `Box::into_raw` で生成したポインタ。
        unsafe { drop(Box::from_raw(machine)) };
    }
}

/// 大域環境と読み込み済みモジュールを凍結し、実行の段階へ移行する。
/// すでに凍結済みの場合はそのまま `true` を返す。
///
/// # Safety
/// `machine` は NULL または排他的に利用できる有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_freeze(machine: *mut RinMachine) -> bool {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return false;
    };
    if let RinMachineState::Building(builder) = m.state_ref()
        && let Err(err) = builder.check_freeze()
    {
        make_err_res(m, err);
        return false;
    }
    match m.state.take().expect("RinMachine state present") {
        RinMachineState::Building(builder) => {
            let frozen = builder.freeze().expect("check_freeze already succeeded");
            m.state = Some(RinMachineState::Frozen(frozen));
            true
        }
        RinMachineState::Frozen(frozen) => {
            m.state = Some(RinMachineState::Frozen(frozen));
            true
        }
    }
}

/// 組み込み数学関数の FMA 設定を変更する。
///
/// # Safety
/// `machine` は NULL または排他的に利用できる有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_set_math_fma(machine: *mut RinMachine, mode: i32) -> bool {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return false;
    };
    let parsed = match mode {
        0 => MathFma::Strict,
        1 => MathFma::Fused,
        _ => {
            host_error(m, "invalid math fma mode");
            return false;
        }
    };
    match m.state_mut() {
        RinMachineState::Building(b) => b.set_math_fma(parsed),
        RinMachineState::Frozen(f) => f.set_math_fma(parsed),
    }
    true
}

/// 現在の組み込み数学関数の FMA 設定を返す。`machine` が NULL なら `Strict` を返す。
///
/// # Safety
/// `machine` は NULL または有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_math_fma(machine: *const RinMachine) -> RinMathFma {
    // SAFETY: 呼び出し側が NULL または有効なポインタを保証する。
    let Some(m) = (unsafe { machine.as_ref() }) else {
        return RinMathFma::Strict;
    };
    let mode = match m.state_ref() {
        RinMachineState::Building(b) => b.math_fma(),
        RinMachineState::Frozen(f) => f.math_fma(),
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
    machine: *mut RinMachine,
    name: *const c_char,
    val: *const RinValue,
) -> bool {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return false;
    };
    if name.is_null() || val.is_null() {
        host_error(m, "null pointer argument");
        return false;
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(key) = unsafe { CStr::from_ptr(name) }.to_str() else {
        host_error(m, "invalid UTF-8 binding name");
        return false;
    };
    // SAFETY: 呼び出し側が有効な `RinValue` を保証する。
    let v = unsafe { &(*val).0 };
    let res = match m.state_mut() {
        RinMachineState::Building(b) => b.bind(key, v),
        RinMachineState::Frozen(_) => Err(Error::runtime(
            "env.frozen",
            Span::default(),
            "cannot bind in a frozen environment",
        )),
    };
    match res {
        Ok(()) => true,
        Err(err) => {
            make_err_res(m, err);
            false
        }
    }
}

/// # Safety
/// 各ポインタは NULL または有効でなければならない。非 NULL の `source` は NUL 終端。
/// machine は排他的に利用する。不正 UTF-8 と NULL 引数はエラーとして扱う。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_eval(
    machine: *mut RinMachine,
    source: *const c_char,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if source.is_null() {
        return host_error(m, "null pointer argument");
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(src) = unsafe { CStr::from_ptr(source) }.to_str() else {
        return host_error(m, "invalid UTF-8 source");
    };

    let res = match m.state_mut() {
        RinMachineState::Building(b) => b.eval(src),
        RinMachineState::Frozen(f) => f.eval(src),
    };
    match res {
        Ok(val) => RinEvalResult {
            ok: true,
            value: Box::into_raw(Box::new(RinValue(val))),
        },
        Err(err) => make_err_res(m, err),
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
    machine: *mut RinMachine,
    fn_name: *const c_char,
    args: *const *const RinValue,
    argc: usize,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if fn_name.is_null() || (argc > 0 && args.is_null()) {
        return host_error(m, "null pointer argument");
    }
    let Ok(name) = (unsafe { CStr::from_ptr(fn_name) }).to_str() else {
        return host_error(m, "invalid UTF-8 function name");
    };
    let mut owned_args = Vec::with_capacity(argc);
    if argc > 0 {
        // SAFETY: 呼び出し側が `args` に `argc` 個の要素があることを保証する。
        let slice = unsafe { std::slice::from_raw_parts(args, argc) };
        for &ptr in slice {
            if ptr.is_null() {
                return host_error(m, "null argument value");
            }
            // SAFETY: 呼び出し側が有効な `RinValue` ポインタを保証する。
            owned_args.push(unsafe { &(*ptr).0 }.clone());
        }
    }
    let res = match m.state_mut() {
        RinMachineState::Building(b) => b.call(name, &owned_args),
        RinMachineState::Frozen(f) => f.call(name, &owned_args),
    };
    match res {
        Ok(val) => RinEvalResult {
            ok: true,
            value: Box::into_raw(Box::new(RinValue(val))),
        },
        Err(err) => make_err_res(m, err),
    }
}

/// ホスト提供の UTF-8 ソースから名前付きモジュールを読み込む。
///
/// # Safety
/// machine は NULL または排他的に利用できる有効なポインタとする。
/// mod_name と source は NULL または有効な NUL 終端文字列。不正 UTF-8 と NULL 引数はエラー。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_load(
    machine: *mut RinMachine,
    mod_name: *const c_char,
    source: *const c_char,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if mod_name.is_null() || source.is_null() {
        return host_error(m, "null pointer argument");
    }
    let (Ok(name), Ok(src)) = (
        unsafe { CStr::from_ptr(mod_name) }.to_str(),
        unsafe { CStr::from_ptr(source) }.to_str(),
    ) else {
        return host_error(m, "invalid UTF-8 argument");
    };
    let res = match m.state_mut() {
        RinMachineState::Building(b) => b.load_module(name, src),
        RinMachineState::Frozen(_) => Err(Error::runtime(
            "env.frozen",
            Span::default(),
            "cannot load module in a frozen environment",
        )),
    };
    match res {
        Ok(()) => RinEvalResult {
            ok: true,
            value: std::ptr::null_mut(),
        },
        Err(err) => make_err_res(m, err),
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
    machine: *mut RinMachine,
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
        return host_error(m, "null pointer argument");
    }
    let (Ok(mname), Ok(fname)) = (
        unsafe { CStr::from_ptr(mod_name) }.to_str(),
        unsafe { CStr::from_ptr(fn_name) }.to_str(),
    ) else {
        return host_error(m, "invalid UTF-8 argument");
    };
    let mut owned_args = Vec::with_capacity(argc);
    if argc > 0 {
        // SAFETY: 呼び出し側が `args` に `argc` 個の要素があることを保証する。
        let slice = unsafe { std::slice::from_raw_parts(args, argc) };
        for &ptr in slice {
            if ptr.is_null() {
                return host_error(m, "null argument value");
            }
            // SAFETY: 呼び出し側が有効な `RinValue` ポインタを保証する。
            owned_args.push(unsafe { &(*ptr).0 }.clone());
        }
    }
    let res = match m.state_mut() {
        RinMachineState::Building(b) => b.call_in(mname, fname, &owned_args),
        RinMachineState::Frozen(f) => f.call_in(mname, fname, &owned_args),
    };
    match res {
        Ok(val) => RinEvalResult {
            ok: true,
            value: Box::into_raw(Box::new(RinValue(val))),
        },
        Err(err) => make_err_res(m, err),
    }
}

/// 直近の対象操作のエラーを借用する。エラーなし、または NULL machine なら NULL。
/// 次の rin_eval / rin_load / rin_call / rin_call_in / rin_bind / rin_freeze / rin_set_math_fma、または rin_free まで有効。
/// この取得関数や rin_res_free はエラーを無効化しない。ホストは解放しない。
///
/// # Safety
/// `machine` は NULL または有効な RinMachine を指し、対象操作と同時に呼ばないこと。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_last_error(machine: *const RinMachine) -> *const RinError {
    // SAFETY: 呼び出し側が NULL または有効なポインタを保証する。
    let Some(m) = (unsafe { machine.as_ref() }) else {
        return std::ptr::null();
    };
    m.last_error
        .as_ref()
        .map_or(std::ptr::null(), |err| &err.view)
}

/// # Safety
/// `res` は `rin_eval` 等が返した有効な `RinEvalResult` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_res_free(res: *mut RinEvalResult) {
    if res.is_null() {
        return;
    }
    // SAFETY: 呼び出し側が有効な `res` を保証する。
    let r = unsafe { &mut *res };
    if !r.value.is_null() {
        // SAFETY: `r.value` は `Box::into_raw` で確保したポインタ。
        unsafe { drop(Box::from_raw(r.value)) };
        r.value = std::ptr::null_mut();
    }
}
