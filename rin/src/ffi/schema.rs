//! C FFI 向けのスキーマ解析・検証 API を実装する。

use std::ffi::{CStr, c_char};

use super::RinValue;
use crate::schema::Schema;

pub struct RinSchema(pub Schema);

/// スキーマ DSL 文字列を解析して `RinSchema*` を返す。構文エラー時は `NULL` を返す。
///
/// # Safety
/// `src` は有効な NUL 終端 UTF-8 文字列でなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_schema(src: *const c_char) -> *mut RinSchema {
    if src.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(text) = unsafe { CStr::from_ptr(src) }.to_str() else {
        return std::ptr::null_mut();
    };
    match Schema::parse(text) {
        Ok(s) => Box::into_raw(Box::new(RinSchema(s))),
        Err(_) => std::ptr::null_mut(),
    }
}

/// # Safety
/// `schema` は `NULL` または `rin_schema` が返したポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_schema_free(schema: *mut RinSchema) {
    if !schema.is_null() {
        // SAFETY: `schema` は `Box::into_raw` で確保したポインタ。
        unsafe { drop(Box::from_raw(schema)) };
    }
}

/// `val` がコンパイル済み `RinSchema*` に適合するか検証する。
///
/// # Safety
/// `val` と `schema` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_check(val: *const RinValue, schema: *const RinSchema) -> bool {
    if val.is_null() || schema.is_null() {
        return false;
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let v = unsafe { &(*val).0 };
    let s = unsafe { &(*schema).0 };
    s.check(v).is_ok()
}

/// `schema_src` の解析と `val` の検証を 1 回で行う。
///
/// # Safety
/// `val` は有効な `RinValue*`、`schema_src` は有効な NUL 終端 UTF-8 文字列でなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_check_str(val: *const RinValue, schema_src: *const c_char) -> bool {
    if val.is_null() || schema_src.is_null() {
        return false;
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let Ok(text) = unsafe { CStr::from_ptr(schema_src) }.to_str() else {
        return false;
    };
    let Ok(schema) = Schema::parse(text) else {
        return false;
    };
    // SAFETY: 呼び出し側が有効な `RinValue*` を保証する。
    schema.check(unsafe { &(*val).0 }).is_ok()
}
