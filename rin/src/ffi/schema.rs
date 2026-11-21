use std::ffi::{CStr, c_char};

use super::RinValue;
use crate::schema::Schema;

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
