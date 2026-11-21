use super::{RinTag, RinValue};
use crate::value::Value;
use std::ffi::{CStr, c_char};

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
pub unsafe extern "C" fn rin_field(val: *const RinValue, field: *const c_char) -> *mut RinValue {
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
pub unsafe extern "C" fn rin_variant_name(val: *const RinValue, out_len: *mut usize) -> *const u8 {
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
        Value::Variant { data: Some(v), .. } => Some((**v).clone()),
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
pub unsafe extern "C" fn rin_cons(head: *const RinValue, tail: *const RinValue) -> *mut RinValue {
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
