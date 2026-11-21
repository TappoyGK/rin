use std::alloc::Layout;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::NonNull;

use crate::alloc::{Allocator, HeapAllocator};
use crate::error::Limits;
use crate::eval::Machine;
use crate::schema::Schema;
use crate::value::Value;

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
            error_message: CString::new("invalid UTF-8 function name").unwrap().into_raw(),
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

#[unsafe(no_mangle)]
pub extern "C" fn rin_int(n: i64) -> *mut RinValue {
    Box::into_raw(Box::new(RinValue(Value::Int(n))))
}

#[unsafe(no_mangle)]
pub extern "C" fn rin_float(x: f64) -> *mut RinValue {
    Box::into_raw(Box::new(RinValue(Value::Float(x))))
}

#[unsafe(no_mangle)]
pub extern "C" fn rin_bool(b: bool) -> *mut RinValue {
    Box::into_raw(Box::new(RinValue(Value::Bool(b))))
}

/// # Safety
/// `s` must be a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_str(s: *const c_char) -> *mut RinValue {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees NUL-terminated string.
    let Ok(text) = unsafe { CStr::from_ptr(s) }.to_str() else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(RinValue(Value::str(text))))
}

/// # Safety
/// `ptr` must be valid for `len` `i64` elements (or `len == 0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_i64s(ptr: *const i64, len: usize) -> *mut RinValue {
    let items = if len == 0 {
        Vec::new()
    } else {
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: Caller guarantees `ptr` is valid for `len` elements.
        unsafe { std::slice::from_raw_parts(ptr, len) }
            .iter()
            .copied()
            .map(Value::Int)
            .collect()
    };
    Box::into_raw(Box::new(RinValue(Value::array(items))))
}

/// # Safety
/// `ptr` must be valid for `len` `f64` elements (or `len == 0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_f64s(ptr: *const f64, len: usize) -> *mut RinValue {
    let items = if len == 0 {
        Vec::new()
    } else {
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: Caller guarantees `ptr` is valid for `len` elements.
        unsafe { std::slice::from_raw_parts(ptr, len) }
            .iter()
            .copied()
            .map(Value::Float)
            .collect()
    };
    Box::into_raw(Box::new(RinValue(Value::array(items))))
}

/// # Safety
/// `val` must be null or a pointer created by Rin FFI functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_drop(val: *mut RinValue) {
    if !val.is_null() {
        // SAFETY: `val` was allocated with `Box::into_raw`.
        unsafe { drop(Box::from_raw(val)) };
    }
}

/// # Safety
/// `val` must be a valid `RinValue` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_tag(val: *const RinValue) -> RinTag {
    if val.is_null() {
        return RinTag::Void;
    }
    // SAFETY: Caller guarantees `val` is valid.
    match unsafe { &(*val).0 } {
        Value::Void => RinTag::Void,
        Value::Int(_) => RinTag::Int,
        Value::Float(_) => RinTag::Float,
        Value::Bool(_) => RinTag::Bool,
        Value::Char(_) => RinTag::Char,
        Value::Str(_) => RinTag::Str,
        Value::List(_) => RinTag::List,
        Value::Array(_) => RinTag::Array,
        Value::Tuple(_) => RinTag::Tuple,
        Value::Struct { .. } => RinTag::Struct,
        Value::Variant { .. } => RinTag::Variant,
        Value::None => RinTag::None,
        Value::Some(_) => RinTag::Some,
        Value::Ok(_) => RinTag::Ok,
        Value::Err(_) => RinTag::Err,
        Value::Builtin(_) => RinTag::Fn,
    }
}

/// # Safety
/// `val` and `out` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_int(val: *const RinValue, out: *mut i64) -> bool {
    if val.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: Caller guarantees valid pointers.
    if let Value::Int(n) = unsafe { &(*val).0 } {
        unsafe { *out = *n };
        return true;
    }
    false
}

/// # Safety
/// `val` and `out` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_float(val: *const RinValue, out: *mut f64) -> bool {
    if val.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: Caller guarantees valid pointers.
    if let Value::Float(x) = unsafe { &(*val).0 } {
        unsafe { *out = *x };
        return true;
    }
    false
}

/// Returns the length of a `List`, `Array`, or `Tuple` (0 otherwise).
///
/// # Safety
/// `val` must be a valid `RinValue` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_len(val: *const RinValue) -> usize {
    if val.is_null() {
        return 0;
    }
    // SAFETY: Caller guarantees valid pointer.
    match unsafe { &(*val).0 } {
        Value::List(xs) => xs.len(),
        Value::Array(xs) | Value::Tuple(xs) => xs.len(),
        _ => 0,
    }
}

/// # Safety
/// `val` and `out` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_bool(val: *const RinValue, out: *mut bool) -> bool {
    if val.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: Caller guarantees valid pointers.
    if let Value::Bool(b) = unsafe { &(*val).0 } {
        unsafe { *out = *b };
        return true;
    }
    false
}

/// Borrows the UTF-8 bytes of a `Str` value (valid while `val` is alive).
/// Returns NULL if `val` is not a string.
///
/// # Safety
/// `val` and `out_len` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_str(val: *const RinValue, out_len: *mut usize) -> *const u8 {
    if val.is_null() || out_len.is_null() {
        return std::ptr::null();
    }
    // SAFETY: Caller guarantees valid pointers.
    if let Value::Str(s) = unsafe { &(*val).0 } {
        unsafe { *out_len = s.len() };
        return s.as_ptr();
    }
    std::ptr::null()
}

/// Returns a newly allocated `RinValue*` for `field` on a `Struct`, or NULL if missing/non-struct.
///
/// # Safety
/// `val` and `field` must be valid pointers (`field` NUL-terminated UTF-8).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_field(
    val: *const RinValue,
    field: *const c_char,
) -> *mut RinValue {
    if val.is_null() || field.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees valid NUL-terminated string.
    let Ok(name) = unsafe { CStr::from_ptr(field) }.to_str() else {
        return std::ptr::null_mut();
    };
    // SAFETY: Caller guarantees valid pointer.
    let Value::Struct { fields, .. } = (unsafe { &(*val).0 }) else {
        return std::ptr::null_mut();
    };
    match fields.get(name) {
        Some(v) => Box::into_raw(Box::new(RinValue(v.clone()))),
        None => std::ptr::null_mut(),
    }
}

/// Borrows the UTF-8 bytes of a `Variant`'s variant name (valid while `val` is alive).
/// Returns NULL if `val` is not a `Variant`.
///
/// # Safety
/// `val` and `out_len` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_variant_name(
    val: *const RinValue,
    out_len: *mut usize,
) -> *const u8 {
    if val.is_null() || out_len.is_null() {
        return std::ptr::null();
    }
    // SAFETY: Caller guarantees valid pointers.
    if let Value::Variant { variant, .. } = unsafe { &(*val).0 } {
        unsafe { *out_len = variant.len() };
        return variant.as_ptr();
    }
    std::ptr::null()
}

/// Returns a newly allocated `RinValue*` for the inner value of `Some`, `Ok`, `Err`, or `Variant` payload.
/// Returns NULL if `val` has no payload.
///
/// # Safety
/// `val` must be a valid `RinValue` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_unwrap(val: *const RinValue) -> *mut RinValue {
    if val.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees valid pointer.
    let inner = match unsafe { &(*val).0 } {
        Value::Some(v) | Value::Ok(v) | Value::Err(v) => Some((**v).clone()),
        Value::Variant {
            data: Some(v), ..
        } => Some((**v).clone()),
        _ => None,
    };
    match inner {
        Some(v) => Box::into_raw(Box::new(RinValue(v))),
        None => std::ptr::null_mut(),
    }
}

/// Returns a newly allocated `RinValue*` for element `index` of a `Tuple` or `Array`, or NULL if out of range.
///
/// # Safety
/// `val` must be a valid `RinValue` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_tuple_get(val: *const RinValue, index: usize) -> *mut RinValue {
    if val.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees valid pointer.
    match unsafe { &(*val).0 } {
        Value::Tuple(xs) | Value::Array(xs) => match xs.get(index) {
            Some(v) => Box::into_raw(Box::new(RinValue(v.clone()))),
            None => std::ptr::null_mut(),
        },
        _ => std::ptr::null_mut(),
    }
}

/// Copies elements of an `array` of `i64` into `out_ptr` (up to `out_cap` elements).
/// Returns the number of elements copied, or `usize::MAX` if `val` is not an `i64` array.
///
/// # Safety
/// `val` must be valid and `out_ptr` must be writable for `out_cap` `i64` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_copy_i64s(
    val: *const RinValue,
    out_ptr: *mut i64,
    out_cap: usize,
) -> usize {
    if val.is_null() {
        return usize::MAX;
    }
    // SAFETY: Caller guarantees valid pointer.
    let Value::Array(xs) = (unsafe { &(*val).0 }) else {
        return usize::MAX;
    };
    let count = xs.len().min(out_cap);
    if count > 0 && out_ptr.is_null() {
        return usize::MAX;
    }
    for (i, item) in xs.iter().take(count).enumerate() {
        let Value::Int(n) = item else {
            return usize::MAX;
        };
        // SAFETY: `i < count <= out_cap`.
        unsafe { *out_ptr.add(i) = *n };
    }
    count
}

/// Copies elements of an `array` of `f64` into `out_ptr` (up to `out_cap` elements).
/// Returns the number of elements copied, or `usize::MAX` if `val` is not an `f64` array.
///
/// # Safety
/// `val` must be valid and `out_ptr` must be writable for `out_cap` `f64` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_copy_f64s(
    val: *const RinValue,
    out_ptr: *mut f64,
    out_cap: usize,
) -> usize {
    if val.is_null() {
        return usize::MAX;
    }
    // SAFETY: Caller guarantees valid pointer.
    let Value::Array(xs) = (unsafe { &(*val).0 }) else {
        return usize::MAX;
    };
    let count = xs.len().min(out_cap);
    if count > 0 && out_ptr.is_null() {
        return usize::MAX;
    }
    for (i, item) in xs.iter().take(count).enumerate() {
        let Value::Float(x) = item else {
            return usize::MAX;
        };
        // SAFETY: `i < count <= out_cap`.
        unsafe { *out_ptr.add(i) = *x };
    }
    count
}

#[unsafe(no_mangle)]
pub extern "C" fn rin_nil() -> *mut RinValue {
    Box::into_raw(Box::new(RinValue(Value::empty_list())))
}

/// Prepends `head` onto `tail` (`tail` must be a `List`), returning a new `List` in O(1).
///
/// # Safety
/// `head` and `tail` must be valid `RinValue` pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_cons(
    head: *const RinValue,
    tail: *const RinValue,
) -> *mut RinValue {
    if head.is_null() || tail.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees valid pointers.
    let h = unsafe { &(*head).0 }.clone();
    // SAFETY: Caller guarantees valid pointer.
    let Value::List(t) = (unsafe { &(*tail).0 }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(RinValue(Value::cons(h, t.clone()))))
}

#[unsafe(no_mangle)]
pub extern "C" fn rin_none() -> *mut RinValue {
    Box::into_raw(Box::new(RinValue(Value::None)))
}

/// # Safety
/// `val` must be a valid `RinValue` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_some(val: *const RinValue) -> *mut RinValue {
    if val.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees valid pointer.
    let inner = unsafe { &(*val).0 }.clone();
    Box::into_raw(Box::new(RinValue(Value::Some(std::rc::Rc::new(inner)))))
}

/// # Safety
/// `name` must be a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_struct(name: *const c_char) -> *mut RinValue {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees valid NUL-terminated string.
    let Ok(s) = unsafe { CStr::from_ptr(name) }.to_str() else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(RinValue(Value::structure(
        s,
        std::collections::BTreeMap::new(),
    ))))
}

/// Sets `field` on a `Struct` `RinValue` (copies `val`).
///
/// # Safety
/// `s`, `field` (NUL-terminated UTF-8), and `val` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_struct_set(
    s: *mut RinValue,
    field: *const c_char,
    val: *const RinValue,
) -> bool {
    if s.is_null() || field.is_null() || val.is_null() {
        return false;
    }
    // SAFETY: Caller guarantees valid NUL-terminated string.
    let Ok(fname) = unsafe { CStr::from_ptr(field) }.to_str() else {
        return false;
    };
    // SAFETY: Caller guarantees valid pointers.
    let v = unsafe { &(*val).0 }.clone();
    let Value::Struct { fields, .. } = (unsafe { &mut (*s).0 }) else {
        return false;
    };
    std::rc::Rc::make_mut(fields).insert(fname.to_string(), v);
    true
}

/// Constructs a `Variant` value (`data` may be NULL for unit variants).
///
/// # Safety
/// `type_name` and `variant` must be valid NUL-terminated UTF-8 strings; `data` is NULL or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_variant(
    type_name: *const c_char,
    variant: *const c_char,
    data: *const RinValue,
) -> *mut RinValue {
    if type_name.is_null() || variant.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees valid NUL-terminated strings.
    let (Ok(tname), Ok(vname)) = (
        unsafe { CStr::from_ptr(type_name) }.to_str(),
        unsafe { CStr::from_ptr(variant) }.to_str(),
    ) else {
        return std::ptr::null_mut();
    };
    let payload = if data.is_null() {
        None
    } else {
        // SAFETY: Caller guarantees valid pointer.
        Some(unsafe { &(*data).0 }.clone())
    };
    Box::into_raw(Box::new(RinValue(Value::variant(tname, vname, payload))))
}

/// Returns a newly allocated `RinValue*` for the head (`car`) of a `List`, or NULL if empty/non-list.
///
/// # Safety
/// `val` must be a valid `RinValue` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_car(val: *const RinValue) -> *mut RinValue {
    if val.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees valid pointer.
    let Value::List(xs) = (unsafe { &(*val).0 }) else {
        return std::ptr::null_mut();
    };
    match xs.head() {
        Some(h) => Box::into_raw(Box::new(RinValue(h.clone()))),
        None => std::ptr::null_mut(),
    }
}

/// Returns a newly allocated `RinValue*` for the tail (`cdr`) of a `List` in O(1), or NULL if non-list.
///
/// # Safety
/// `val` must be a valid `RinValue` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_cdr(val: *const RinValue) -> *mut RinValue {
    if val.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees valid pointer.
    let Value::List(xs) = (unsafe { &(*val).0 }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(RinValue(Value::List(xs.tail()))))
}

pub struct RinSchema(pub Schema);

/// Parses a Rin schema DSL string into an opaque `RinSchema*`, or returns NULL on parse error.
///
/// # Safety
/// `src` must be a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_schema(src: *const c_char) -> *mut RinSchema {
    if src.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees NUL-terminated string.
    let Ok(text) = unsafe { CStr::from_ptr(src) }.to_str() else {
        return std::ptr::null_mut();
    };
    match Schema::parse(text) {
        Ok(s) => Box::into_raw(Box::new(RinSchema(s))),
        Err(_) => std::ptr::null_mut(),
    }
}

/// # Safety
/// `schema` must be null or a pointer returned by `rin_schema`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_schema_free(schema: *mut RinSchema) {
    if !schema.is_null() {
        // SAFETY: `schema` was allocated with `Box::into_raw`.
        unsafe { drop(Box::from_raw(schema)) };
    }
}

/// Validates `val` against a compiled `RinSchema*`.
///
/// # Safety
/// `val` and `schema` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_check(val: *const RinValue, schema: *const RinSchema) -> bool {
    if val.is_null() || schema.is_null() {
        return false;
    }
    // SAFETY: Caller guarantees valid pointers.
    let v = unsafe { &(*val).0 };
    let s = unsafe { &(*schema).0 };
    s.check(v).is_ok()
}

/// Parses `schema_src` and validates `val` in one step.
///
/// # Safety
/// `val` must be a valid `RinValue*` and `schema_src` a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_check_str(val: *const RinValue, schema_src: *const c_char) -> bool {
    if val.is_null() || schema_src.is_null() {
        return false;
    }
    // SAFETY: Caller guarantees valid pointers.
    let Ok(text) = unsafe { CStr::from_ptr(schema_src) }.to_str() else {
        return false;
    };
    let Ok(schema) = Schema::parse(text) else {
        return false;
    };
    // SAFETY: Caller guarantees valid `RinValue*`.
    schema.check(unsafe { &(*val).0 }).is_ok()
}
