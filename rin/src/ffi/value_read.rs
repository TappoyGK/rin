//! C FFI 向けのゼロヒープ `RinValue` 読み取り・コレクション要素取得 API。

use std::ffi::{CStr, c_char};

use super::{Rin, RinTag, RinValue};
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
        ValueTag::Ident => RinTag::Ident,
    }
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

/// `Ident` 値の UTF-8 バイト列を借用する（`val` の生存中のみ有効）。
/// 識別子でない場合は `NULL` を返す。
///
/// # Safety
/// `machine`・`val`・`out_len` は有効なポインタでなければならない。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_as_ident(
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
    if let Some(s) = m.state_ref().value_ref(v.0).as_ident() {
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
