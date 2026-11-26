//! C FFI 向けのゼロヒープ `RinValue` 生成・解放 API。

use std::ffi::{CStr, c_char};

use super::{Rin, RinState, RinValue};
use crate::value_ref::Val;

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
        m.record_host_error("null pointer argument");
        return std::ptr::null_mut();
    }
    let Ok(text) = (unsafe { CStr::from_ptr(s) }).to_str() else {
        m.record_host_error("invalid UTF-8 string");
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
/// `machine` は有効な `Rin` ポインタ、`s` は有効な NUL 終端 UTF-8 識別子文字列でなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_ident(machine: *mut Rin, s: *const c_char) -> *mut RinValue {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    if s.is_null() {
        m.record_host_error("null pointer argument");
        return std::ptr::null_mut();
    }
    let Ok(text) = (unsafe { CStr::from_ptr(s) }).to_str() else {
        m.record_host_error("invalid UTF-8 string");
        return std::ptr::null_mut();
    };
    if !crate::lexer::is_valid_ident(text) {
        m.record_host_error("invalid identifier");
        return std::ptr::null_mut();
    }
    let res = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => b.val_ident(text).map_err(|e| m.last_error.set_from_ref(e)),
        RinState::Frozen(f) => f.val_ident(text).map_err(|e| m.last_error.set_from_ref(e)),
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
            m.record_host_error("null pointer argument");
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
            m.record_host_error("null pointer argument");
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
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    let (Some(h), Some(t)) = (unsafe { head.as_ref() }, unsafe { tail.as_ref() }) else {
        m.record_host_error("null pointer argument");
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
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return std::ptr::null_mut();
    };
    let Some(v) = (unsafe { val.as_ref() }) else {
        m.record_host_error("null pointer argument");
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
        m.record_host_error("null pointer argument");
        return std::ptr::null_mut();
    }
    let Ok(s) = (unsafe { CStr::from_ptr(name) }).to_str() else {
        m.record_host_error("invalid UTF-8 struct name");
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
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return false;
    };
    let (Some(sr), Some(vr)) = (unsafe { s.as_mut() }, unsafe { val.as_ref() }) else {
        m.record_host_error("null pointer argument");
        return false;
    };
    if !unsafe { m.owns_handle(sr as *const RinValue) } {
        m.record_host_error("invalid struct handle");
        return false;
    }
    if field.is_null() {
        m.record_host_error("null pointer argument");
        return false;
    }
    let Ok(fname) = (unsafe { CStr::from_ptr(field) }).to_str() else {
        m.record_host_error("invalid UTF-8 field name");
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
        m.record_host_error("null pointer argument");
        return std::ptr::null_mut();
    }
    let (Ok(tname), Ok(vname)) = (
        unsafe { CStr::from_ptr(type_name) }.to_str(),
        unsafe { CStr::from_ptr(variant) }.to_str(),
    ) else {
        m.record_host_error("invalid UTF-8 variant name");
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

/// # Safety
/// `machine` は有効な `Rin` ポインタ、`val` は `NULL` または同じ `machine` が返した所有ハンドルでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_drop(machine: *mut Rin, val: *mut RinValue) {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return;
    };
    m.free_handle(val);
}
