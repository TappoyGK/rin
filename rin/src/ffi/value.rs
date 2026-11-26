//! C FFI 向けのゼロヒープ `RinValue` 生成・読み取り・コレクション操作を実装する。

use std::ffi::{CStr, c_char};

use super::{Rin, RinState, RinTag, RinValue};
use crate::value_ref::{Val, ValueTag};

fn tag_from_value_tag(t: ValueTag) -> RinTag {
    match t {
        ValueTag::Void => RinTag::Void,
        ValueTag::Int => RinTag::Int,
        ValueTag::Float => RinTag::Float,
        ValueTag::Bool => RinTag::Bool,
        ValueTag::Char => RinTag::Char,
        ValueTag::Str => RinTag::Str,
        ValueTag::List => RinTag::List,
        ValueTag::Array => RinTag::Array,
        ValueTag::Tuple => RinTag::Tuple,
        ValueTag::Struct => RinTag::Struct,
        ValueTag::Variant => RinTag::Variant,
        ValueTag::None => RinTag::None,
        ValueTag::Some => RinTag::Some,
        ValueTag::Ok => RinTag::Ok,
        ValueTag::Err => RinTag::Err,
        ValueTag::Fn => RinTag::Fn,
    }
}

/// # Safety
/// `machine` は有効な `Rin` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_int(machine: *mut Rin, n: i64) -> *mut RinValue {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    let v = match m.state_ref() {
        RinState::Building(b) => b.val_int(n),
        RinState::Frozen(f) => f.val_int(n),
    };
    m.alloc_handle(v)
}

/// # Safety
/// `machine` は有効な `Rin` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_float(machine: *mut Rin, x: f64) -> *mut RinValue {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    let v = match m.state_ref() {
        RinState::Building(b) => b.val_float(x),
        RinState::Frozen(f) => f.val_float(x),
    };
    m.alloc_handle(v)
}

/// # Safety
/// `machine` は有効な `Rin` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_bool(machine: *mut Rin, b: bool) -> *mut RinValue {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    let v = match m.state_ref() {
        RinState::Building(bld) => bld.val_bool(b),
        RinState::Frozen(f) => f.val_bool(b),
    };
    m.alloc_handle(v)
}

/// # Safety
/// `machine` は有効な `Rin` ポインタ、`s` は有効な NUL 終端 UTF-8 文字列でなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_str(machine: *mut Rin, s: *const c_char) -> *mut RinValue {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    if s.is_null() {
        return std::ptr::null_mut();
    }
    let Ok(text) = (unsafe { CStr::from_ptr(s) }).to_str() else {
        return std::ptr::null_mut();
    };
    let res = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => b.val_str(text).map_err(|e| m.last_error.set_from_ref(e)),
        RinState::Frozen(f) => f.val_str(text).map_err(|e| m.last_error.set_from_ref(e)),
    };
    match res {
        Ok(v) => m.alloc_handle(v),
        Err(()) => {
            m.has_error = true;
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `machine` は有効な `Rin` ポインタ、`ptr` は `len` 個の `i64` 要素を読み取れる有効なポインタでなければならない（`len == 0` を除く）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_i64s(machine: *mut Rin, ptr: *const i64, len: usize) -> *mut RinValue {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    let items: &[i64] = if len == 0 {
        &[]
    } else {
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        unsafe { std::slice::from_raw_parts(ptr, len) }
    };
    let res = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => b
            .val_i64_array(items)
            .map_err(|e| m.last_error.set_from_ref(e)),
        RinState::Frozen(f) => f
            .val_i64_array(items)
            .map_err(|e| m.last_error.set_from_ref(e)),
    };
    match res {
        Ok(v) => m.alloc_handle(v),
        Err(()) => {
            m.has_error = true;
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `machine` は有効な `Rin` ポインタ、`ptr` は `len` 個の `f64` 要素を読み取れる有効なポインタでなければならない（`len == 0` を除く）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_f64s(machine: *mut Rin, ptr: *const f64, len: usize) -> *mut RinValue {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    let items: &[f64] = if len == 0 {
        &[]
    } else {
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        unsafe { std::slice::from_raw_parts(ptr, len) }
    };
    let res = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => b
            .val_f64_array(items)
            .map_err(|e| m.last_error.set_from_ref(e)),
        RinState::Frozen(f) => f
            .val_f64_array(items)
            .map_err(|e| m.last_error.set_from_ref(e)),
    };
    match res {
        Ok(v) => m.alloc_handle(v),
        Err(()) => {
            m.has_error = true;
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `machine` は有効な `Rin` ポインタ、`val` は `NULL` または同じ `machine` が返した所有ハンドルでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_drop(machine: *mut Rin, val: *mut RinValue) {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return;
    };
    m.free_handle(val);
}

/// # Safety
/// `machine` と `val` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_tag(machine: *const Rin, val: *const RinValue) -> RinTag {
    let (Some(m), Some(v)) = (unsafe { machine.as_ref() }, unsafe { val.as_ref() }) else {
        return RinTag::Void;
    };
    tag_from_value_tag(m.state_ref().value_ref(v.0).tag())
}

/// # Safety
/// `machine`・`val`・`out` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_int(
    machine: *const Rin,
    val: *const RinValue,
    out: *mut i64,
) -> bool {
    let (Some(m), Some(v)) = (unsafe { machine.as_ref() }, unsafe { val.as_ref() }) else {
        return false;
    };
    if out.is_null() {
        return false;
    }
    if let Some(n) = m.state_ref().value_ref(v.0).as_int() {
        unsafe { *out = n };
        return true;
    }
    false
}

/// # Safety
/// `machine`・`val`・`out` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_float(
    machine: *const Rin,
    val: *const RinValue,
    out: *mut f64,
) -> bool {
    let (Some(m), Some(v)) = (unsafe { machine.as_ref() }, unsafe { val.as_ref() }) else {
        return false;
    };
    if out.is_null() {
        return false;
    }
    if let Some(x) = m.state_ref().value_ref(v.0).as_float() {
        unsafe { *out = x };
        return true;
    }
    false
}

/// `List`・`Array`・`Tuple` の長さを返す（それ以外は 0）。
///
/// # Safety
/// `machine` と `val` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_len(machine: *const Rin, val: *const RinValue) -> usize {
    let (Some(m), Some(v)) = (unsafe { machine.as_ref() }, unsafe { val.as_ref() }) else {
        return 0;
    };
    let vr = m.state_ref().value_ref(v.0);
    if let Some(it) = vr.as_list() {
        it.len()
    } else if let Some(s) = vr.as_array() {
        s.len()
    } else if let Some(s) = vr.as_tuple() {
        s.len()
    } else {
        0
    }
}

/// # Safety
/// `machine`・`val`・`out` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_bool(
    machine: *const Rin,
    val: *const RinValue,
    out: *mut bool,
) -> bool {
    let (Some(m), Some(v)) = (unsafe { machine.as_ref() }, unsafe { val.as_ref() }) else {
        return false;
    };
    if out.is_null() {
        return false;
    }
    if let Some(b) = m.state_ref().value_ref(v.0).as_bool() {
        unsafe { *out = b };
        return true;
    }
    false
}

/// `Str` 値の UTF-8 バイト列を借用する（`val` の生存中のみ有効）。
/// 文字列でない場合は `NULL` を返す。
///
/// # Safety
/// `machine`・`val`・`out_len` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_str(
    machine: *const Rin,
    val: *const RinValue,
    out_len: *mut usize,
) -> *const u8 {
    let (Some(m), Some(v)) = (unsafe { machine.as_ref() }, unsafe { val.as_ref() }) else {
        return std::ptr::null();
    };
    if out_len.is_null() {
        return std::ptr::null();
    }
    if let Some(s) = m.state_ref().value_ref(v.0).as_str() {
        unsafe { *out_len = s.len() };
        return s.as_ptr();
    }
    std::ptr::null()
}

/// `Struct` の `field` を指す新しい所有 `RinValue*` を返す。存在しないか構造体でない場合は `NULL` を返す。
///
/// # Safety
/// `machine`・`val`・`field` は有効なポインタでなければならない（`field` は NUL 終端 UTF-8）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_field(
    machine: *mut Rin,
    val: *const RinValue,
    field: *const c_char,
) -> *mut RinValue {
    let (Some(m), Some(v)) = (unsafe { machine.as_mut() }, unsafe { val.as_ref() }) else {
        return std::ptr::null_mut();
    };
    if field.is_null() {
        return std::ptr::null_mut();
    }
    let Ok(name) = (unsafe { CStr::from_ptr(field) }).to_str() else {
        return std::ptr::null_mut();
    };
    let child = {
        let vr = m.state_ref().value_ref(v.0);
        let Some(sr) = vr.as_struct() else {
            return std::ptr::null_mut();
        };
        sr.field(name).map(|f| Val(f.val))
    };
    let Some(c) = child else {
        return std::ptr::null_mut();
    };
    m.state_mut().retain_val(c);
    m.alloc_handle(c)
}

/// `Variant` のバリアント名 UTF-8 バイト列を借用する（`val` の生存中のみ有効）。
/// `Variant` でない場合は `NULL` を返す。
///
/// # Safety
/// `machine`・`val`・`out_len` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_variant_name(
    machine: *const Rin,
    val: *const RinValue,
    out_len: *mut usize,
) -> *const u8 {
    let (Some(m), Some(v)) = (unsafe { machine.as_ref() }, unsafe { val.as_ref() }) else {
        return std::ptr::null();
    };
    if out_len.is_null() {
        return std::ptr::null();
    }
    if let Some(vr) = m.state_ref().value_ref(v.0).as_variant() {
        let name = vr.variant();
        unsafe { *out_len = name.len() };
        return name.as_ptr();
    }
    std::ptr::null()
}

/// `Some`・`Ok`・`Err` または `Variant` ペイロードの内側の値を指す新しい所有 `RinValue*` を返す。
/// 値を持たない場合は `NULL` を返す。
///
/// # Safety
/// `machine` と `val` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_unwrap(machine: *mut Rin, val: *const RinValue) -> *mut RinValue {
    let (Some(m), Some(v)) = (unsafe { machine.as_mut() }, unsafe { val.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let inner = {
        let vr = m.state_ref().value_ref(v.0);
        vr.as_some()
            .or_else(|| vr.as_ok())
            .or_else(|| vr.as_err())
            .or_else(|| vr.as_variant().and_then(|var| var.data()))
            .map(|x| Val(x.val))
    };
    let Some(c) = inner else {
        return std::ptr::null_mut();
    };
    m.state_mut().retain_val(c);
    m.alloc_handle(c)
}

/// `Tuple` または `Array` の `index` 番目要素を指す新しい所有 `RinValue*` を返す。範囲外なら `NULL` を返す。
///
/// # Safety
/// `machine` と `val` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_tuple_get(
    machine: *mut Rin,
    val: *const RinValue,
    index: usize,
) -> *mut RinValue {
    let (Some(m), Some(v)) = (unsafe { machine.as_mut() }, unsafe { val.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let elem = {
        let vr = m.state_ref().value_ref(v.0);
        vr.as_tuple()
            .or_else(|| vr.as_array())
            .and_then(|s| s.get(index))
            .map(|x| Val(x.val))
    };
    let Some(c) = elem else {
        return std::ptr::null_mut();
    };
    m.state_mut().retain_val(c);
    m.alloc_handle(c)
}

/// `i64` 配列の要素を最大 `out_cap` 個まで `out_ptr` へコピーする。
/// コピーした要素数を返し、`i64` 配列でない場合は `usize::MAX` を返す。
///
/// # Safety
/// `machine`・`val` は有効であり、`out_ptr` は `out_cap` 個の `i64` 要素を書き込めなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_copy_i64s(
    machine: *const Rin,
    val: *const RinValue,
    out_ptr: *mut i64,
    out_cap: usize,
) -> usize {
    let (Some(m), Some(v)) = (unsafe { machine.as_ref() }, unsafe { val.as_ref() }) else {
        return usize::MAX;
    };
    let vr = m.state_ref().value_ref(v.0);
    let Some(arr) = vr.as_array() else {
        return usize::MAX;
    };
    let count = arr.len().min(out_cap);
    if count > 0 && out_ptr.is_null() {
        return usize::MAX;
    }
    for i in 0..count {
        let Some(n) = arr.get(i).and_then(|x| x.as_int()) else {
            return usize::MAX;
        };
        unsafe { *out_ptr.add(i) = n };
    }
    count
}

/// `f64` 配列の要素を最大 `out_cap` 個まで `out_ptr` へコピーする。
/// コピーした要素数を返し、`f64` 配列でない場合は `usize::MAX` を返す。
///
/// # Safety
/// `machine`・`val` は有効であり、`out_ptr` は `out_cap` 個の `f64` 要素を書き込めなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_copy_f64s(
    machine: *const Rin,
    val: *const RinValue,
    out_ptr: *mut f64,
    out_cap: usize,
) -> usize {
    let (Some(m), Some(v)) = (unsafe { machine.as_ref() }, unsafe { val.as_ref() }) else {
        return usize::MAX;
    };
    let vr = m.state_ref().value_ref(v.0);
    let Some(arr) = vr.as_array() else {
        return usize::MAX;
    };
    let count = arr.len().min(out_cap);
    if count > 0 && out_ptr.is_null() {
        return usize::MAX;
    }
    for i in 0..count {
        let Some(x) = arr.get(i).and_then(|x| x.as_float()) else {
            return usize::MAX;
        };
        unsafe { *out_ptr.add(i) = x };
    }
    count
}

/// # Safety
/// `machine` は有効な `Rin` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_nil(machine: *mut Rin) -> *mut RinValue {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    m.alloc_handle(Val::empty_list())
}

/// `tail`（`List`）の先頭に `head` を追加した新しい `List` を O(1) で生成する。
///
/// # Safety
/// `machine`・`head`・`tail` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_cons(
    machine: *mut Rin,
    head: *const RinValue,
    tail: *const RinValue,
) -> *mut RinValue {
    let (Some(m), Some(h), Some(t)) = (
        unsafe { machine.as_mut() },
        unsafe { head.as_ref() },
        unsafe { tail.as_ref() },
    ) else {
        return std::ptr::null_mut();
    };
    let res = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => b
            .val_cons(h.0, t.0)
            .map_err(|e| m.last_error.set_from_ref(e)),
        RinState::Frozen(f) => f
            .val_cons(h.0, t.0)
            .map_err(|e| m.last_error.set_from_ref(e)),
    };
    match res {
        Ok(v) => m.alloc_handle(v),
        Err(()) => {
            m.has_error = true;
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `machine` は有効な `Rin` ポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_none(machine: *mut Rin) -> *mut RinValue {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    let v = match m.state_ref() {
        RinState::Building(b) => b.val_none(),
        RinState::Frozen(f) => f.val_none(),
    };
    m.alloc_handle(v)
}

/// # Safety
/// `machine` と `val` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_some(machine: *mut Rin, val: *const RinValue) -> *mut RinValue {
    let (Some(m), Some(v)) = (unsafe { machine.as_mut() }, unsafe { val.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let res = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => b.val_some(v.0).map_err(|e| m.last_error.set_from_ref(e)),
        RinState::Frozen(f) => f.val_some(v.0).map_err(|e| m.last_error.set_from_ref(e)),
    };
    match res {
        Ok(out) => m.alloc_handle(out),
        Err(()) => {
            m.has_error = true;
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `machine` は有効な `Rin` ポインタ、`name` は有効な NUL 終端 UTF-8 文字列でなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_struct(machine: *mut Rin, name: *const c_char) -> *mut RinValue {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    if name.is_null() {
        return std::ptr::null_mut();
    }
    let Ok(s) = (unsafe { CStr::from_ptr(name) }).to_str() else {
        return std::ptr::null_mut();
    };
    let res = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => b
            .val_struct(s, &[])
            .map_err(|e| m.last_error.set_from_ref(e)),
        RinState::Frozen(f) => f
            .val_struct(s, &[])
            .map_err(|e| m.last_error.set_from_ref(e)),
    };
    match res {
        Ok(v) => m.alloc_handle(v),
        Err(()) => {
            m.has_error = true;
            std::ptr::null_mut()
        }
    }
}

/// `Struct` `RinValue` の `field` に `val` を設定する。
///
/// # Safety
/// `machine`・`s`・`field`（NUL 終端 UTF-8）・`val` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_struct_set(
    machine: *mut Rin,
    s: *mut RinValue,
    field: *const c_char,
    val: *const RinValue,
) -> bool {
    let (Some(m), Some(sr), Some(vr)) =
        (unsafe { machine.as_mut() }, unsafe { s.as_mut() }, unsafe {
            val.as_ref()
        })
    else {
        return false;
    };
    if field.is_null() {
        return false;
    }
    let Ok(fname) = (unsafe { CStr::from_ptr(field) }).to_str() else {
        return false;
    };
    let old = sr.0;
    let res = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => b
            .val_struct_set(old, fname, vr.0)
            .map_err(|e| m.last_error.set_from_ref(e)),
        RinState::Frozen(f) => f
            .val_struct_set(old, fname, vr.0)
            .map_err(|e| m.last_error.set_from_ref(e)),
    };
    match res {
        Ok(new_val) => {
            sr.0 = new_val;
            m.state_mut().release_val(old);
            true
        }
        Err(()) => {
            m.has_error = true;
            false
        }
    }
}

/// `Variant` 値を生成する（ペイロードなしの場合は `data` に `NULL` を渡す）。
///
/// # Safety
/// `machine` は有効な `Rin` ポインタ、`type_name` と `variant` は有効な NUL 終端 UTF-8 文字列、`data` は `NULL` または有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_variant(
    machine: *mut Rin,
    type_name: *const c_char,
    variant: *const c_char,
    data: *const RinValue,
) -> *mut RinValue {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    if type_name.is_null() || variant.is_null() {
        return std::ptr::null_mut();
    }
    let (Ok(tname), Ok(vname)) = (
        unsafe { CStr::from_ptr(type_name) }.to_str(),
        unsafe { CStr::from_ptr(variant) }.to_str(),
    ) else {
        return std::ptr::null_mut();
    };
    let payload = if data.is_null() {
        None
    } else {
        Some(unsafe { (*data).0 })
    };
    let res = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => b
            .val_variant(tname, vname, payload)
            .map_err(|e| m.last_error.set_from_ref(e)),
        RinState::Frozen(f) => f
            .val_variant(tname, vname, payload)
            .map_err(|e| m.last_error.set_from_ref(e)),
    };
    match res {
        Ok(v) => m.alloc_handle(v),
        Err(()) => {
            m.has_error = true;
            std::ptr::null_mut()
        }
    }
}

/// `List` の先頭（`car`）を指す新しい所有 `RinValue*` を返す。空またはリストでない場合は `NULL` を返す。
///
/// # Safety
/// `machine` と `val` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_car(machine: *mut Rin, val: *const RinValue) -> *mut RinValue {
    let (Some(m), Some(v)) = (unsafe { machine.as_mut() }, unsafe { val.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let head = {
        let vr = m.state_ref().value_ref(v.0);
        vr.as_list()
            .and_then(|mut it| it.next())
            .map(|x| Val(x.val))
    };
    let Some(h) = head else {
        return std::ptr::null_mut();
    };
    m.state_mut().retain_val(h);
    m.alloc_handle(h)
}

/// `List` の残り（`cdr`）を指す新しい所有 `RinValue*` を O(1) で返す。リストでない場合は `NULL` を返す。
///
/// # Safety
/// `machine` と `val` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_cdr(machine: *mut Rin, val: *const RinValue) -> *mut RinValue {
    let (Some(m), Some(v)) = (unsafe { machine.as_mut() }, unsafe { val.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let tail = {
        let vr = m.state_ref().value_ref(v.0);
        match vr.as_list() {
            Some(mut it) => {
                let _ = it.next();
                Val(it.cur_rval())
            }
            None => return std::ptr::null_mut(),
        }
    };
    m.state_mut().retain_val(tail);
    m.alloc_handle(tail)
}
