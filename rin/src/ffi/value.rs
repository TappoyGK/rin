//! C FFI 向けの `RinValue` 生成・読み取り・コレクション操作を実装する。

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
/// `s` は有効な NUL 終端 UTF-8 文字列でなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_str(s: *const c_char) -> *mut RinValue {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(text) = unsafe { CStr::from_ptr(s) }.to_str() else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(RinValue(Value::str(text))))
}

/// # Safety
/// `ptr` は `len` 個の `i64` 要素を読み取れる有効なポインタでなければならない（`len == 0` を除く）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_i64s(ptr: *const i64, len: usize) -> *mut RinValue {
    let items = if len == 0 {
        Vec::new()
    } else {
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: 呼び出し側が `ptr` に `len` 個の要素があることを保証する。
        unsafe { std::slice::from_raw_parts(ptr, len) }
            .iter()
            .copied()
            .map(Value::Int)
            .collect()
    };
    Box::into_raw(Box::new(RinValue(Value::array(items))))
}

/// # Safety
/// `ptr` は `len` 個の `f64` 要素を読み取れる有効なポインタでなければならない（`len == 0` を除く）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_f64s(ptr: *const f64, len: usize) -> *mut RinValue {
    let items = if len == 0 {
        Vec::new()
    } else {
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: 呼び出し側が `ptr` に `len` 個の要素があることを保証する。
        unsafe { std::slice::from_raw_parts(ptr, len) }
            .iter()
            .copied()
            .map(Value::Float)
            .collect()
    };
    Box::into_raw(Box::new(RinValue(Value::array(items))))
}

/// # Safety
/// `val` は `NULL` または Rin FFI 関数が生成したポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_drop(val: *mut RinValue) {
    if !val.is_null() {
        // SAFETY: `val` は `Box::into_raw` で確保したポインタ。
        unsafe { drop(Box::from_raw(val)) };
    }
}

/// # Safety
/// `val` は有効な `RinValue` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_tag(val: *const RinValue) -> RinTag {
    if val.is_null() {
        return RinTag::Void;
    }
    // SAFETY: 呼び出し側が有効な `val` を保証する。
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
/// `val` と `out` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_int(val: *const RinValue, out: *mut i64) -> bool {
    if val.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    if let Value::Int(n) = unsafe { &(*val).0 } {
        unsafe { *out = *n };
        return true;
    }
    false
}

/// # Safety
/// `val` と `out` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_float(val: *const RinValue, out: *mut f64) -> bool {
    if val.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    if let Value::Float(x) = unsafe { &(*val).0 } {
        unsafe { *out = *x };
        return true;
    }
    false
}

/// `List`・`Array`・`Tuple` の長さを返す（それ以外は 0）。
///
/// # Safety
/// `val` は有効な `RinValue` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_len(val: *const RinValue) -> usize {
    if val.is_null() {
        return 0;
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    match unsafe { &(*val).0 } {
        Value::List(xs) => xs.len(),
        Value::Array(xs) | Value::Tuple(xs) => xs.len(),
        _ => 0,
    }
}

/// # Safety
/// `val` と `out` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_bool(val: *const RinValue, out: *mut bool) -> bool {
    if val.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    if let Value::Bool(b) = unsafe { &(*val).0 } {
        unsafe { *out = *b };
        return true;
    }
    false
}

/// `Str` 値の UTF-8 バイト列を借用する（`val` の生存中のみ有効）。
/// 文字列でない場合は `NULL` を返す。
///
/// # Safety
/// `val` と `out_len` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_str(val: *const RinValue, out_len: *mut usize) -> *const u8 {
    if val.is_null() || out_len.is_null() {
        return std::ptr::null();
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    if let Value::Str(s) = unsafe { &(*val).0 } {
        unsafe { *out_len = s.len() };
        return s.as_ptr();
    }
    std::ptr::null()
}

/// `Struct` の `field` を指す新しい `RinValue*` を返す。存在しないか構造体でない場合は `NULL` を返す。
///
/// # Safety
/// `val` と `field` は有効なポインタでなければならない（`field` は NUL 終端 UTF-8）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_field(val: *const RinValue, field: *const c_char) -> *mut RinValue {
    if val.is_null() || field.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Caller guarantees valid NUL-terminated string.
    let Ok(name) = unsafe { CStr::from_ptr(field) }.to_str() else {
        return std::ptr::null_mut();
    };
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let Value::Struct { fields, .. } = (unsafe { &(*val).0 }) else {
        return std::ptr::null_mut();
    };
    match fields.get(name) {
        Some(v) => Box::into_raw(Box::new(RinValue(v.clone()))),
        None => std::ptr::null_mut(),
    }
}

/// `Variant` のバリアント名 UTF-8 バイト列を借用する（`val` の生存中のみ有効）。
/// `Variant` でない場合は `NULL` を返す。
///
/// # Safety
/// `val` と `out_len` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_variant_name(val: *const RinValue, out_len: *mut usize) -> *const u8 {
    if val.is_null() || out_len.is_null() {
        return std::ptr::null();
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    if let Value::Variant { variant, .. } = unsafe { &(*val).0 } {
        unsafe { *out_len = variant.len() };
        return variant.as_ptr();
    }
    std::ptr::null()
}

/// `Some`・`Ok`・`Err` または `Variant` ペイロードの内側の値を指す新しい `RinValue*` を返す。
/// 値を持たない場合は `NULL` を返す。
///
/// # Safety
/// `val` は有効な `RinValue` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_unwrap(val: *const RinValue) -> *mut RinValue {
    if val.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
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

/// `Tuple` または `Array` の `index` 番目要素を指す新しい `RinValue*` を返す。範囲外なら `NULL` を返す。
///
/// # Safety
/// `val` は有効な `RinValue` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_tuple_get(val: *const RinValue, index: usize) -> *mut RinValue {
    if val.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    match unsafe { &(*val).0 } {
        Value::Tuple(xs) | Value::Array(xs) => match xs.get(index) {
            Some(v) => Box::into_raw(Box::new(RinValue(v.clone()))),
            None => std::ptr::null_mut(),
        },
        _ => std::ptr::null_mut(),
    }
}

/// `i64` 配列の要素を最大 `out_cap` 個まで `out_ptr` へコピーする。
/// コピーした要素数を返し、`i64` 配列でない場合は `usize::MAX` を返す。
///
/// # Safety
/// `val` は有効であり、`out_ptr` は `out_cap` 個の `i64` 要素を書き込めなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_copy_i64s(
    val: *const RinValue,
    out_ptr: *mut i64,
    out_cap: usize,
) -> usize {
    if val.is_null() {
        return usize::MAX;
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
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
        // SAFETY: `i < count <= out_cap` が成り立つ。
        unsafe { *out_ptr.add(i) = *n };
    }
    count
}

/// `f64` 配列の要素を最大 `out_cap` 個まで `out_ptr` へコピーする。
/// コピーした要素数を返し、`f64` 配列でない場合は `usize::MAX` を返す。
///
/// # Safety
/// `val` は有効であり、`out_ptr` は `out_cap` 個の `f64` 要素を書き込めなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_copy_f64s(
    val: *const RinValue,
    out_ptr: *mut f64,
    out_cap: usize,
) -> usize {
    if val.is_null() {
        return usize::MAX;
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
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
        // SAFETY: `i < count <= out_cap` が成り立つ。
        unsafe { *out_ptr.add(i) = *x };
    }
    count
}

#[unsafe(no_mangle)]
pub extern "C" fn rin_nil() -> *mut RinValue {
    Box::into_raw(Box::new(RinValue(Value::empty_list())))
}

/// `tail`（`List`）の先頭に `head` を追加した新しい `List` を O(1) で生成する。
///
/// # Safety
/// `head` と `tail` は有効な `RinValue` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_cons(head: *const RinValue, tail: *const RinValue) -> *mut RinValue {
    if head.is_null() || tail.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let h = unsafe { &(*head).0 }.clone();
    // SAFETY: 呼び出し側が有効なポインタを保証する。
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
/// `val` は有効な `RinValue` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_some(val: *const RinValue) -> *mut RinValue {
    if val.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let inner = unsafe { &(*val).0 }.clone();
    Box::into_raw(Box::new(RinValue(Value::Some(std::rc::Rc::new(inner)))))
}

/// # Safety
/// `name` は有効な NUL 終端 UTF-8 文字列でなければならない。
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

/// `Struct` `RinValue` の `field` に `val` の複製を設定する。
///
/// # Safety
/// `s`・`field`（NUL 終端 UTF-8）・`val` は有効なポインタでなければならない。
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
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let v = unsafe { &(*val).0 }.clone();
    let Value::Struct { fields, .. } = (unsafe { &mut (*s).0 }) else {
        return false;
    };
    std::rc::Rc::make_mut(fields).insert(fname.to_string(), v);
    true
}

/// `Variant` 値を生成する（ペイロードなしの場合は `data` に `NULL` を渡す）。
///
/// # Safety
/// `type_name` と `variant` は有効な NUL 終端 UTF-8 文字列、`data` は `NULL` または有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_variant(
    type_name: *const c_char,
    variant: *const c_char,
    data: *const RinValue,
) -> *mut RinValue {
    if type_name.is_null() || variant.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let (Ok(tname), Ok(vname)) = (
        unsafe { CStr::from_ptr(type_name) }.to_str(),
        unsafe { CStr::from_ptr(variant) }.to_str(),
    ) else {
        return std::ptr::null_mut();
    };
    let payload = if data.is_null() {
        None
    } else {
        // SAFETY: 呼び出し側が有効なポインタを保証する。
        Some(unsafe { &(*data).0 }.clone())
    };
    Box::into_raw(Box::new(RinValue(Value::variant(tname, vname, payload))))
}

/// `List` の先頭（`car`）を指す新しい `RinValue*` を返す。空またはリストでない場合は `NULL` を返す。
///
/// # Safety
/// `val` は有効な `RinValue` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_car(val: *const RinValue) -> *mut RinValue {
    if val.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let Value::List(xs) = (unsafe { &(*val).0 }) else {
        return std::ptr::null_mut();
    };
    match xs.head() {
        Some(h) => Box::into_raw(Box::new(RinValue(h.clone()))),
        None => std::ptr::null_mut(),
    }
}

/// `List` の残り（`cdr`）を指す新しい `RinValue*` を O(1) で返す。リストでない場合は `NULL` を返す。
///
/// # Safety
/// `val` は有効な `RinValue` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_cdr(val: *const RinValue) -> *mut RinValue {
    if val.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: 呼び出し側が有効なポインタを保証する。
    let Value::List(xs) = (unsafe { &(*val).0 }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(RinValue(Value::List(xs.tail()))))
}
