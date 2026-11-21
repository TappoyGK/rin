//! C 言語から Rin を利用するための FFI 境界とマシン操作 API を実装する。

use std::alloc::Layout;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::NonNull;

use crate::alloc::{Allocator, HeapAllocator};
use crate::error::{Error, Limits};
use crate::eval::Machine;
use crate::value::Value;

mod schema;
mod value;

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

pub struct RinMachine {
    // `Pool::drop` が `_alloc` を呼ぶため、`_alloc` より先に `machine` を破棄する。
    machine: Machine<'static>,
    last_backtrace: Option<CString>,
    _alloc: Box<CAllocator>,
}

fn make_err_res(m: &mut RinMachine, err: Error) -> RinEvalResult {
    let bt = err.format_backtrace();
    m.last_backtrace = if bt.is_empty() {
        None
    } else {
        CString::new(bt).ok()
    };
    let msg = CString::new(err.message).unwrap_or_else(|_| CString::new("error").unwrap());
    RinEvalResult {
        ok: false,
        value: std::ptr::null_mut(),
        error_message: msg.into_raw(),
        error_line: err.at.line,
        error_col: err.at.col,
    }
}

pub struct RinValue(pub Value);

#[repr(C)]
pub struct RinEvalResult {
    pub ok: bool,
    pub value: *mut RinValue,
    pub error_message: *mut c_char,
    pub error_line: usize,
    pub error_col: usize,
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
    let machine = Machine::new(limits.into(), alloc_ref);
    Box::into_raw(Box::new(RinMachine {
        machine,
        last_backtrace: None,
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

/// # Safety
/// `machine`・`name`・`val` は有効なポインタでなければならない（`name` は NUL 終端 UTF-8）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_bind(
    machine: *mut RinMachine,
    name: *const c_char,
    val: *const RinValue,
) -> bool {
    if machine.is_null() || name.is_null() || val.is_null() {
        return false;
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let m = unsafe { &mut *machine };
    m.last_backtrace = None;
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(key) = unsafe { CStr::from_ptr(name) }.to_str() else {
        return false;
    };
    // SAFETY: 呼び出し側が有効な `RinValue` を保証する。
    let v = unsafe { &(*val).0 };
    m.machine.bind(key, v).is_ok()
}

/// # Safety
/// `machine`・`source` は有効なポインタでなければならない（`source` は NUL 終端 UTF-8）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_eval(
    machine: *mut RinMachine,
    source: *const c_char,
) -> RinEvalResult {
    if machine.is_null() || source.is_null() {
        return RinEvalResult {
            ok: false,
            value: std::ptr::null_mut(),
            error_message: CString::new("null pointer argument").unwrap().into_raw(),
            error_line: 0,
            error_col: 0,
        };
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let m = unsafe { &mut *machine };
    m.last_backtrace = None;
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(src) = unsafe { CStr::from_ptr(source) }.to_str() else {
        return RinEvalResult {
            ok: false,
            value: std::ptr::null_mut(),
            error_message: CString::new("invalid UTF-8 source").unwrap().into_raw(),
            error_line: 0,
            error_col: 0,
        };
    };

    match m.machine.eval(src) {
        Ok(val) => {
            m.last_backtrace = None;
            RinEvalResult {
                ok: true,
                value: Box::into_raw(Box::new(RinValue(val))),
                error_message: std::ptr::null_mut(),
                error_line: 0,
                error_col: 0,
            }
        }
        Err(err) => make_err_res(m, err),
    }
}

/// 定義済みの Rin 関数をソース再解析なしで呼び出す。
///
/// # Safety
/// `machine`・`fn_name`（NUL 終端 UTF-8）・`args`（`argc > 0` 時）は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_call(
    machine: *mut RinMachine,
    fn_name: *const c_char,
    args: *const *const RinValue,
    argc: usize,
) -> RinEvalResult {
    if machine.is_null() || fn_name.is_null() || (argc > 0 && args.is_null()) {
        return RinEvalResult {
            ok: false,
            value: std::ptr::null_mut(),
            error_message: CString::new("null pointer argument").unwrap().into_raw(),
            error_line: 0,
            error_col: 0,
        };
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let m = unsafe { &mut *machine };
    m.last_backtrace = None;
    let Ok(name) = (unsafe { CStr::from_ptr(fn_name) }).to_str() else {
        return RinEvalResult {
            ok: false,
            value: std::ptr::null_mut(),
            error_message: CString::new("invalid UTF-8 function name")
                .unwrap()
                .into_raw(),
            error_line: 0,
            error_col: 0,
        };
    };
    let mut owned_args = Vec::with_capacity(argc);
    if argc > 0 {
        // SAFETY: 呼び出し側が `args` に `argc` 個の要素があることを保証する。
        let slice = unsafe { std::slice::from_raw_parts(args, argc) };
        for &ptr in slice {
            if ptr.is_null() {
                return RinEvalResult {
                    ok: false,
                    value: std::ptr::null_mut(),
                    error_message: CString::new("null argument value").unwrap().into_raw(),
                    error_line: 0,
                    error_col: 0,
                };
            }
            // SAFETY: 呼び出し側が有効な `RinValue` ポインタを保証する。
            owned_args.push(unsafe { &(*ptr).0 }.clone());
        }
    }
    match m.machine.call(name, &owned_args) {
        Ok(val) => {
            m.last_backtrace = None;
            RinEvalResult {
                ok: true,
                value: Box::into_raw(Box::new(RinValue(val))),
                error_message: std::ptr::null_mut(),
                error_line: 0,
                error_col: 0,
            }
        }
        Err(err) => make_err_res(m, err),
    }
}

/// ホスト提供の UTF-8 ソースから名前付きモジュールを読み込む。
///
/// # Safety
/// `machine`・`mod_name`・`source` は有効な NUL 終端 UTF-8 ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_load(
    machine: *mut RinMachine,
    mod_name: *const c_char,
    source: *const c_char,
) -> RinEvalResult {
    if machine.is_null() || mod_name.is_null() || source.is_null() {
        return RinEvalResult {
            ok: false,
            value: std::ptr::null_mut(),
            error_message: CString::new("null pointer argument").unwrap().into_raw(),
            error_line: 0,
            error_col: 0,
        };
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let m = unsafe { &mut *machine };
    m.last_backtrace = None;
    let (Ok(name), Ok(src)) = (
        unsafe { CStr::from_ptr(mod_name) }.to_str(),
        unsafe { CStr::from_ptr(source) }.to_str(),
    ) else {
        return RinEvalResult {
            ok: false,
            value: std::ptr::null_mut(),
            error_message: CString::new("invalid UTF-8 argument").unwrap().into_raw(),
            error_line: 0,
            error_col: 0,
        };
    };
    match m.machine.load_module(name, src) {
        Ok(()) => {
            m.last_backtrace = None;
            RinEvalResult {
                ok: true,
                value: std::ptr::null_mut(),
                error_message: std::ptr::null_mut(),
                error_line: 0,
                error_col: 0,
            }
        }
        Err(err) => make_err_res(m, err),
    }
}

/// 読み込み済みモジュール `mod_name` 内の公開関数 `fn_name` を呼び出す。
///
/// # Safety
/// `machine`・`mod_name`・`fn_name`・`args`（`argc > 0` 時）は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_call_in(
    machine: *mut RinMachine,
    mod_name: *const c_char,
    fn_name: *const c_char,
    args: *const *const RinValue,
    argc: usize,
) -> RinEvalResult {
    if machine.is_null() || mod_name.is_null() || fn_name.is_null() || (argc > 0 && args.is_null())
    {
        return RinEvalResult {
            ok: false,
            value: std::ptr::null_mut(),
            error_message: CString::new("null pointer argument").unwrap().into_raw(),
            error_line: 0,
            error_col: 0,
        };
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let m = unsafe { &mut *machine };
    m.last_backtrace = None;
    let (Ok(mname), Ok(fname)) = (
        unsafe { CStr::from_ptr(mod_name) }.to_str(),
        unsafe { CStr::from_ptr(fn_name) }.to_str(),
    ) else {
        return RinEvalResult {
            ok: false,
            value: std::ptr::null_mut(),
            error_message: CString::new("invalid UTF-8 argument").unwrap().into_raw(),
            error_line: 0,
            error_col: 0,
        };
    };
    let mut owned_args = Vec::with_capacity(argc);
    if argc > 0 {
        // SAFETY: 呼び出し側が `args` に `argc` 個の要素があることを保証する。
        let slice = unsafe { std::slice::from_raw_parts(args, argc) };
        for &ptr in slice {
            if ptr.is_null() {
                return RinEvalResult {
                    ok: false,
                    value: std::ptr::null_mut(),
                    error_message: CString::new("null argument value").unwrap().into_raw(),
                    error_line: 0,
                    error_col: 0,
                };
            }
            // SAFETY: 呼び出し側が有効な `RinValue` ポインタを保証する。
            owned_args.push(unsafe { &(*ptr).0 }.clone());
        }
    }
    match m.machine.call_in(mname, fname, &owned_args) {
        Ok(val) => {
            m.last_backtrace = None;
            RinEvalResult {
                ok: true,
                value: Box::into_raw(Box::new(RinValue(val))),
                error_message: std::ptr::null_mut(),
                error_line: 0,
                error_col: 0,
            }
        }
        Err(err) => make_err_res(m, err),
    }
}

/// `machine` で直近に発生したエラーの整形済み呼び出し履歴を返す。履歴がない場合は `NULL`。
/// 返されたポインタは次の評価・呼び出しまたは `rin_free` まで有効。
///
/// # Safety
/// `machine` は `NULL` または `rin_new` が返した有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_last_backtrace(machine: *const RinMachine) -> *const c_char {
    if machine.is_null() {
        return std::ptr::null();
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let m = unsafe { &*machine };
    match &m.last_backtrace {
        Some(cs) => cs.as_ptr(),
        None => std::ptr::null(),
    }
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
    if !r.error_message.is_null() {
        // SAFETY: `r.error_message` は `CString::into_raw` で確保したポインタ。
        unsafe { drop(CString::from_raw(r.error_message)) };
        r.error_message = std::ptr::null_mut();
    }
}
