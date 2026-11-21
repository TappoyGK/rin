use std::alloc::Layout;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::NonNull;

use crate::alloc::{Allocator, HeapAllocator};
use crate::error::Limits;
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
            // SAFETY: Caller guarantees `f` and `self.raw.ctx` are valid for the lifetime of the machine.
            let ptr = unsafe { f(self.raw.ctx, layout.size(), layout.align()) };
            NonNull::new(ptr)
        } else {
            HeapAllocator.alloc(layout)
        }
    }

    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        if let Some(f) = self.raw.dealloc {
            // SAFETY: Caller guarantees `f` matches `alloc`.
            unsafe { f(self.raw.ctx, ptr.as_ptr(), layout.size(), layout.align()) };
        } else if self.raw.alloc.is_none() {
            // SAFETY: Allocated via `HeapAllocator` when `self.raw.alloc` is `None`.
            unsafe { HeapAllocator.dealloc(ptr, layout) };
        }
    }
}

pub struct RinMachine {
    // Drop `machine` before `_alloc` because `Pool::drop` calls `_alloc`.
    machine: Machine<'static>,
    _alloc: Box<CAllocator>,
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
    // SAFETY: `boxed_alloc` lives inside `RinMachine` and is dropped after `machine`.
    let alloc_ref: &'static mut dyn Allocator =
        unsafe { &mut *(boxed_alloc.as_mut() as *mut CAllocator as *mut dyn Allocator) };
    let machine = Machine::new(limits.into(), alloc_ref);
    Box::into_raw(Box::new(RinMachine {
        machine,
        _alloc: boxed_alloc,
    }))
}

/// # Safety
/// `machine` must be null or a pointer returned by `rin_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_free(machine: *mut RinMachine) {
    if !machine.is_null() {
        // SAFETY: `machine` was created via `Box::into_raw`.
        unsafe { drop(Box::from_raw(machine)) };
    }
}

/// # Safety
/// `machine`, `name`, and `val` must be valid pointers (`name` NUL-terminated UTF-8).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_bind(
    machine: *mut RinMachine,
    name: *const c_char,
    val: *const RinValue,
) -> bool {
    if machine.is_null() || name.is_null() || val.is_null() {
        return false;
    }
    // SAFETY: Caller guarantees valid pointers.
    let m = unsafe { &mut *machine };
    // SAFETY: Caller guarantees valid NUL-terminated string.
    let Ok(key) = unsafe { CStr::from_ptr(name) }.to_str() else {
        return false;
    };
    // SAFETY: Caller guarantees valid `RinValue`.
    let v = unsafe { &(*val).0 };
    m.machine.bind(key, v).is_ok()
}

/// # Safety
/// `machine` and `source` must be valid pointers (`source` NUL-terminated UTF-8).
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
    // SAFETY: Caller guarantees valid pointers.
    let m = unsafe { &mut *machine };
    // SAFETY: Caller guarantees valid NUL-terminated string.
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
        Ok(val) => RinEvalResult {
            ok: true,
            value: Box::into_raw(Box::new(RinValue(val))),
            error_message: std::ptr::null_mut(),
            error_line: 0,
            error_col: 0,
        },
        Err(err) => {
            let msg = CString::new(err.message).unwrap_or_else(|_| CString::new("error").unwrap());
            RinEvalResult {
                ok: false,
                value: std::ptr::null_mut(),
                error_message: msg.into_raw(),
                error_line: err.at.line,
                error_col: err.at.col,
            }
        }
    }
}

/// Calls a defined Rin function by name with `argc` `RinValue*` arguments (without re-parsing).
///
/// # Safety
/// `machine`, `fn_name` (NUL-terminated UTF-8), and `args` (for `argc > 0`) must be valid pointers.
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
    // SAFETY: Caller guarantees valid pointers.
    let m = unsafe { &mut *machine };
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
        // SAFETY: Caller guarantees `args` has `argc` elements.
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
            // SAFETY: Caller guarantees valid `RinValue` pointer.
            owned_args.push(unsafe { &(*ptr).0 }.clone());
        }
    }
    match m.machine.call(name, &owned_args) {
        Ok(val) => RinEvalResult {
            ok: true,
            value: Box::into_raw(Box::new(RinValue(val))),
            error_message: std::ptr::null_mut(),
            error_line: 0,
            error_col: 0,
        },
        Err(err) => {
            let msg = CString::new(err.message).unwrap_or_else(|_| CString::new("error").unwrap());
            RinEvalResult {
                ok: false,
                value: std::ptr::null_mut(),
                error_message: msg.into_raw(),
                error_line: err.at.line,
                error_col: err.at.col,
            }
        }
    }
}

/// Loads a named module from UTF-8 `source` provided by the host.
///
/// # Safety
/// `machine`, `mod_name`, and `source` must be valid NUL-terminated UTF-8 pointers.
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
    // SAFETY: Caller guarantees valid pointers.
    let m = unsafe { &mut *machine };
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
        Ok(()) => RinEvalResult {
            ok: true,
            value: std::ptr::null_mut(),
            error_message: std::ptr::null_mut(),
            error_line: 0,
            error_col: 0,
        },
        Err(err) => {
            let msg = CString::new(err.message).unwrap_or_else(|_| CString::new("error").unwrap());
            RinEvalResult {
                ok: false,
                value: std::ptr::null_mut(),
                error_message: msg.into_raw(),
                error_line: err.at.line,
                error_col: err.at.col,
            }
        }
    }
}

/// Calls an exported function `fn_name` inside loaded module `mod_name`.
///
/// # Safety
/// `machine`, `mod_name`, `fn_name`, and `args` (for `argc > 0`) must be valid pointers.
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
    // SAFETY: Caller guarantees valid pointers.
    let m = unsafe { &mut *machine };
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
        // SAFETY: Caller guarantees `args` has `argc` elements.
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
            // SAFETY: Caller guarantees valid `RinValue` pointer.
            owned_args.push(unsafe { &(*ptr).0 }.clone());
        }
    }
    match m.machine.call_in(mname, fname, &owned_args) {
        Ok(val) => RinEvalResult {
            ok: true,
            value: Box::into_raw(Box::new(RinValue(val))),
            error_message: std::ptr::null_mut(),
            error_line: 0,
            error_col: 0,
        },
        Err(err) => {
            let msg = CString::new(err.message).unwrap_or_else(|_| CString::new("error").unwrap());
            RinEvalResult {
                ok: false,
                value: std::ptr::null_mut(),
                error_message: msg.into_raw(),
                error_line: err.at.line,
                error_col: err.at.col,
            }
        }
    }
}

/// # Safety
/// `res` must be a valid pointer to a `RinEvalResult` returned by `rin_eval`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_res_free(res: *mut RinEvalResult) {
    if res.is_null() {
        return;
    }
    // SAFETY: Caller guarantees `res` is valid.
    let r = unsafe { &mut *res };
    if !r.value.is_null() {
        // SAFETY: `r.value` was allocated with `Box::into_raw`.
        unsafe { drop(Box::from_raw(r.value)) };
        r.value = std::ptr::null_mut();
    }
    if !r.error_message.is_null() {
        // SAFETY: `r.error_message` was allocated with `CString::into_raw`.
        unsafe { drop(CString::from_raw(r.error_message)) };
        r.error_message = std::ptr::null_mut();
    }
}
