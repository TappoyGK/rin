//! C FFI の評価・モジュール読み込み・関数呼び出し・エラー取得 API。

use std::alloc::Layout;
use std::ffi::{CStr, c_char};
use std::ptr::NonNull;

use crate::alloc::Allocator;
use crate::error::Span;
use crate::value_ref::Val;

use super::{
    Rin, RinBinding, RinError, RinErrorKind, RinEvalResult, RinResolvedBinding, RinState, RinType,
    RinValue, begin_operation,
};

unsafe fn read_c_str<'a>(m: &mut Rin, ptr: *const c_char, label: &str) -> Option<&'a str> {
    if ptr.is_null() {
        m.record_host_error(label);
        return None;
    }
    match unsafe { CStr::from_ptr(ptr) }.to_str() {
        Ok(s) => Some(s),
        Err(_) => {
            m.record_host_error("invalid UTF-8 argument");
            None
        }
    }
}

unsafe fn read_val(m: &mut Rin, ptr: *const RinValue, label: &str) -> Option<Val> {
    if ptr.is_null() {
        m.record_host_error(label);
        return None;
    }
    Some(unsafe { (*ptr).0 })
}

const STACK_FFI_ARGS: usize = 16;

/// # Safety
/// 各ポインタは NULL または有効でなければならない。非 NULL の `name` は NUL 終端。
/// machine は排他的に利用する。不正 UTF-8 と NULL 引数はエラーとして扱う。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_bind(
    machine: *mut Rin,
    name: *const c_char,
    type_expr: *const c_char,
    val: *const RinValue,
) -> bool {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return false;
    };
    if name.is_null() || type_expr.is_null() || val.is_null() {
        m.record_host_error("null pointer argument");
        return false;
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(key) = unsafe { CStr::from_ptr(name) }.to_str() else {
        m.record_host_error("invalid UTF-8 binding name");
        return false;
    };
    let Ok(ty_str) = unsafe { CStr::from_ptr(type_expr) }.to_str() else {
        m.record_host_error("invalid UTF-8 type expression");
        return false;
    };
    // SAFETY: 呼び出し側が有効な `RinValue` を保証する。
    let v = unsafe { (*val).0 };
    let err_recorded = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => match b.bind_val(key, ty_str, v) {
            Ok(()) => return true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                true
            }
        },
        RinState::Frozen(_) => {
            m.record_simple_error(
                "env.frozen",
                RinErrorKind::Runtime,
                Span::default(),
                "cannot bind in a frozen environment",
            );
            return false;
        }
    };
    if err_recorded {
        m.has_error = true;
    }
    false
}

/// 読み込み段階の環境でソースを評価する（`def`・`defn`・`import` 等を許可する）。
///
/// # Safety
/// 各ポインタは NULL または有効でなければならない。非 NULL の `source` は NUL 終端。
/// machine は排他的に利用する。不正 UTF-8 と NULL 引数はエラーとして扱う。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_build(machine: *mut Rin, source: *const c_char) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if source.is_null() {
        m.record_host_error("null pointer argument");
        return RinEvalResult::failure();
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(src) = unsafe { CStr::from_ptr(source) }.to_str() else {
        m.record_host_error("invalid UTF-8 source");
        return RinEvalResult::failure();
    };

    let ok = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => match b.eval_ref(src) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        RinState::Frozen(_) => {
            m.record_simple_error(
                "env.frozen",
                RinErrorKind::Runtime,
                Span::default(),
                "cannot call rin_build on a frozen Rin",
            );
            false
        }
    };
    if ok {
        m.finish_eval_result()
    } else {
        RinEvalResult::failure()
    }
}

/// 凍結された実行段階の環境でソースを評価する。
///
/// # Safety
/// 各ポインタは NULL または有効でなければならない。非 NULL の `source` は NUL 終端。
/// machine は排他的に利用する。不正 UTF-8 と NULL 引数はエラーとして扱う。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_eval(machine: *mut Rin, source: *const c_char) -> RinEvalResult {
    // SAFETY: 呼び出し側が引数の契約を満たしている。
    unsafe { eval_frozen_inner(machine, source, std::ptr::null(), 0, "rin_eval") }
}

/// 凍結された実行段階の環境で、この 1 回の評価にだけ有効な初期束縛を与えてソースを評価する。
///
/// # Safety
/// machine は NULL または排他的に利用できる有効なポインタとする。
/// source は NULL または NUL 終端文字列。bindings は NULL または binding_count 個の要素を持つ配列。
/// 各要素の name は NULL または NUL 終端文字列、value は NULL または有効な RinValue を指す。
/// 不正 UTF-8 と NULL 引数はエラーとして扱う。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_eval_with(
    machine: *mut Rin,
    source: *const c_char,
    bindings: *const RinBinding,
    binding_count: usize,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が引数の契約を満たしている。
    unsafe { eval_frozen_inner(machine, source, bindings, binding_count, "rin_eval_with") }
}

unsafe fn eval_frozen_inner(
    machine: *mut Rin,
    source: *const c_char,
    bindings: *const RinBinding,
    binding_count: usize,
    fn_name: &str,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if source.is_null() || (binding_count > 0 && bindings.is_null()) {
        m.record_host_error("null pointer argument");
        return RinEvalResult::failure();
    }
    // SAFETY: 呼び出し側が有効な NUL 終端文字列を保証する。
    let Ok(src) = unsafe { CStr::from_ptr(source) }.to_str() else {
        m.record_host_error("invalid UTF-8 source");
        return RinEvalResult::failure();
    };

    if matches!(m.state_ref(), RinState::Building(_)) {
        // 先に bindings の引数エラーを検査する。
        if binding_count > 0 {
            let raw_bindings = unsafe { std::slice::from_raw_parts(bindings, binding_count) };
            for b in raw_bindings {
                if b.name.is_null() || b.type_expr.is_null() || b.value.is_null() {
                    m.record_host_error("null pointer argument");
                    return RinEvalResult::failure();
                }
                if unsafe { CStr::from_ptr(b.name) }.to_str().is_err() {
                    m.record_host_error("invalid UTF-8 binding name");
                    return RinEvalResult::failure();
                }
                if unsafe { CStr::from_ptr(b.type_expr) }.to_str().is_err() {
                    m.record_host_error("invalid UTF-8 type expression");
                    return RinEvalResult::failure();
                }
            }
        }
        m.record_simple_error_fmt(
            "env.phase",
            RinErrorKind::Runtime,
            Span::default(),
            format_args!("{fn_name} requires a frozen Rin; call rin_freeze first or use rin_build"),
        );
        return RinEvalResult::failure();
    }

    type BindingEntry<'b> = (&'b str, &'b str, Val);
    let mut stack_buf: [BindingEntry<'_>; STACK_FFI_ARGS] = [("", "", Val::void()); STACK_FFI_ARGS];
    let mut heap_ptr: Option<(NonNull<BindingEntry<'_>>, Layout)> = None;
    let slice_mut: &mut [BindingEntry<'_>] = if binding_count <= STACK_FFI_ARGS {
        &mut stack_buf[..binding_count]
    } else {
        let Some(layout) = Layout::array::<BindingEntry<'_>>(binding_count).ok() else {
            m.record_simple_error(
                "eval.limits",
                RinErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return RinEvalResult::failure();
        };
        let mut calloc = m.alloc;
        let Some(raw) = calloc.alloc(layout) else {
            m.record_simple_error(
                "eval.limits",
                RinErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return RinEvalResult::failure();
        };
        let p = raw.cast::<(&str, &str, Val)>();
        heap_ptr = Some((p, layout));
        // SAFETY: `p` は `binding_count` 個分確保済み。すぐ下で全要素初期化する。
        unsafe {
            for i in 0..binding_count {
                p.as_ptr().add(i).write(("", "", Val::void()));
            }
            std::slice::from_raw_parts_mut(p.as_ptr(), binding_count)
        }
    };

    let mut filled = 0usize;
    if binding_count > 0 {
        // SAFETY: binding_count > 0 かつ bindings 非 NULL。
        let raw_bindings = unsafe { std::slice::from_raw_parts(bindings, binding_count) };
        for (i, b) in raw_bindings.iter().enumerate() {
            if b.name.is_null() || b.type_expr.is_null() || b.value.is_null() {
                for &(_, _, v) in &slice_mut[..filled] {
                    m.state_mut().release_val(v);
                }
                if let Some((p, layout)) = heap_ptr {
                    let mut calloc = m.alloc;
                    unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
                }
                m.record_host_error("null pointer argument");
                return RinEvalResult::failure();
            }
            let Ok(name) = unsafe { CStr::from_ptr(b.name) }.to_str() else {
                for &(_, _, v) in &slice_mut[..filled] {
                    m.state_mut().release_val(v);
                }
                if let Some((p, layout)) = heap_ptr {
                    let mut calloc = m.alloc;
                    unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
                }
                m.record_host_error("invalid UTF-8 binding name");
                return RinEvalResult::failure();
            };
            let Ok(ty_str) = unsafe { CStr::from_ptr(b.type_expr) }.to_str() else {
                for &(_, _, v) in &slice_mut[..filled] {
                    m.state_mut().release_val(v);
                }
                if let Some((p, layout)) = heap_ptr {
                    let mut calloc = m.alloc;
                    unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
                }
                m.record_host_error("invalid UTF-8 type expression");
                return RinEvalResult::failure();
            };
            let v = unsafe { (*b.value).0 };
            // 前回の `last_result` をそのまま引数に渡しても安全にするため retain する。
            m.state_mut().retain_val(v);
            slice_mut[i] = (name, ty_str, v);
            filled += 1;
        }
    }

    let ok = match m.state.as_mut().expect("Rin state present") {
        RinState::Frozen(f) => match f.eval_with_vals(src, slice_mut) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        RinState::Building(_) => unreachable!(),
    };

    for &(_, _, v) in &slice_mut[..filled] {
        m.state_mut().release_val(v);
    }
    if let Some((p, layout)) = heap_ptr {
        let mut calloc = m.alloc;
        unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
    }

    if ok {
        m.finish_eval_result()
    } else {
        RinEvalResult::failure()
    }
}

/// 定義済みの Rin 関数をソース再解析なしで呼び出す。
///
/// # Safety
/// machine は NULL または排他的に利用できる有効なポインタとする。
/// fn_name は NULL または NUL 終端文字列。args は NULL または argc 個の要素を持つ配列。
/// 各引数値は NULL または有効な RinValue を指す。不正 UTF-8 と NULL 引数はエラー。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_call(
    machine: *mut Rin,
    fn_name: *const c_char,
    args: *const *const RinValue,
    argc: usize,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if fn_name.is_null() || (argc > 0 && args.is_null()) {
        m.record_host_error("null pointer argument");
        return RinEvalResult::failure();
    }
    let Ok(name) = (unsafe { CStr::from_ptr(fn_name) }).to_str() else {
        m.record_host_error("invalid UTF-8 function name");
        return RinEvalResult::failure();
    };
    unsafe { call_with_args(m, None, name, args, argc) }
}

/// ホスト提供の UTF-8 ソースから名前付きモジュールを読み込む。
///
/// # Safety
/// machine は NULL または排他的に利用できる有効なポインタとする。
/// mod_name と source は NULL または有効な NUL 終端文字列。不正 UTF-8 と NULL 引数はエラー。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_load(
    machine: *mut Rin,
    mod_name: *const c_char,
    source: *const c_char,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if mod_name.is_null() || source.is_null() {
        m.record_host_error("null pointer argument");
        return RinEvalResult::failure();
    }
    let (Ok(name), Ok(src)) = (
        unsafe { CStr::from_ptr(mod_name) }.to_str(),
        unsafe { CStr::from_ptr(source) }.to_str(),
    ) else {
        m.record_host_error("invalid UTF-8 argument");
        return RinEvalResult::failure();
    };
    let ok = match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => match b.load_module_ref(name, src) {
            Ok(()) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        RinState::Frozen(_) => {
            m.record_simple_error(
                "env.frozen",
                RinErrorKind::Runtime,
                Span::default(),
                "cannot load module in a frozen environment",
            );
            false
        }
    };
    if ok {
        RinEvalResult {
            ok: true,
            value: std::ptr::null(),
        }
    } else {
        RinEvalResult::failure()
    }
}

/// 読み込み済みモジュール `mod_name` 内の公開関数 `fn_name` を呼び出す。
///
/// # Safety
/// machine は NULL または排他的に利用できる有効なポインタとする。
/// mod_name と fn_name は NULL または NUL 終端文字列。args は NULL または argc 個の要素を持つ配列。
/// 各引数値は NULL または有効な RinValue を指す。不正 UTF-8 と NULL 引数はエラー。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_call_in(
    machine: *mut Rin,
    mod_name: *const c_char,
    fn_name: *const c_char,
    args: *const *const RinValue,
    argc: usize,
) -> RinEvalResult {
    // SAFETY: 呼び出し側が machine の有効性と排他的アクセスを保証する。
    let Some(m) = (unsafe { begin_operation(machine) }) else {
        return RinEvalResult::failure();
    };
    if mod_name.is_null() || fn_name.is_null() || (argc > 0 && args.is_null()) {
        m.record_host_error("null pointer argument");
        return RinEvalResult::failure();
    }
    let (Ok(mname), Ok(fname)) = (
        unsafe { CStr::from_ptr(mod_name) }.to_str(),
        unsafe { CStr::from_ptr(fn_name) }.to_str(),
    ) else {
        m.record_host_error("invalid UTF-8 argument");
        return RinEvalResult::failure();
    };
    unsafe { call_with_args(m, Some(mname), fname, args, argc) }
}

unsafe fn call_with_args(
    m: &mut Rin,
    mod_name: Option<&str>,
    fn_name: &str,
    args: *const *const RinValue,
    argc: usize,
) -> RinEvalResult {
    let mut stack_buf: [Val; STACK_FFI_ARGS] = [Val::void(); STACK_FFI_ARGS];
    let mut heap_ptr: Option<(NonNull<Val>, Layout)> = None;
    let slice_mut: &mut [Val] = if argc <= STACK_FFI_ARGS {
        &mut stack_buf[..argc]
    } else {
        let Some(layout) = Layout::array::<Val>(argc).ok() else {
            m.record_simple_error(
                "eval.limits",
                RinErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return RinEvalResult::failure();
        };
        let mut calloc = m.alloc;
        let Some(raw) = calloc.alloc(layout) else {
            m.record_simple_error(
                "eval.limits",
                RinErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return RinEvalResult::failure();
        };
        let p = raw.cast::<Val>();
        heap_ptr = Some((p, layout));
        unsafe {
            for i in 0..argc {
                p.as_ptr().add(i).write(Val::void());
            }
            std::slice::from_raw_parts_mut(p.as_ptr(), argc)
        }
    };

    let mut filled = 0usize;
    if argc > 0 {
        // SAFETY: 呼び出し側が `args` に `argc` 個の要素があることを保証する。
        let raw_args = unsafe { std::slice::from_raw_parts(args, argc) };
        for (i, &ptr) in raw_args.iter().enumerate() {
            if ptr.is_null() {
                for &v in &slice_mut[..filled] {
                    m.state_mut().release_val(v);
                }
                if let Some((p, layout)) = heap_ptr {
                    let mut calloc = m.alloc;
                    unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
                }
                m.record_host_error("null argument value");
                return RinEvalResult::failure();
            }
            let v = unsafe { (*ptr).0 };
            m.state_mut().retain_val(v);
            slice_mut[i] = v;
            filled += 1;
        }
    }

    let ok = match (m.state.as_mut().expect("Rin state present"), mod_name) {
        (RinState::Building(b), None) => match b.call_ref(fn_name, slice_mut) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        (RinState::Frozen(f), None) => match f.call_ref(fn_name, slice_mut) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        (RinState::Building(b), Some(mn)) => match b.call_in_ref(mn, fn_name, slice_mut) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        (RinState::Frozen(f), Some(mn)) => match f.call_in_ref(mn, fn_name, slice_mut) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
    };

    for &v in &slice_mut[..filled] {
        m.state_mut().release_val(v);
    }
    if let Some((p, layout)) = heap_ptr {
        let mut calloc = m.alloc;
        unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
    }

    if ok {
        m.finish_eval_result()
    } else {
        RinEvalResult::failure()
    }
}

/// 型式文字列を事前解決し、`*out` へ [`RinType`] を書き込む（読み込み段階のみ有効）。
///
/// # Safety
/// `machine`・`type_expr`・`out` は有効なポインタ、`type_expr` は NUL 終端 UTF-8 であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_resolve_type(
    machine: *mut Rin,
    type_expr: *const c_char,
    out: *mut RinType,
) -> bool {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return false;
    };
    m.clear_error();
    if out.is_null() {
        m.record_host_error("rin_resolve_type: out is NULL");
        return false;
    }
    let Some(ty_str) = (unsafe { read_c_str(m, type_expr, "rin_resolve_type: type_expr") }) else {
        return false;
    };
    match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => match b.resolve_type(ty_str) {
            Ok(handle) => {
                unsafe {
                    *out = RinType {
                        id: handle.raw_id(),
                        env_id: handle.raw_env_id(),
                    }
                };
                true
            }
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        RinState::Frozen(_) => {
            m.record_simple_error(
                "env.frozen",
                RinErrorKind::Runtime,
                Span::default(),
                "cannot resolve new type on frozen Rin",
            );
            false
        }
    }
}

/// 事前解決済みの [`RinType`] を使って値を大域変数として束縛する。
///
/// # Safety
/// `machine`・`name`・`val` は有効なポインタであること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_bind_resolved(
    machine: *mut Rin,
    name: *const c_char,
    ty: RinType,
    val: *const RinValue,
) -> bool {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return false;
    };
    m.clear_error();
    let Some(name_str) = (unsafe { read_c_str(m, name, "rin_bind_resolved: name") }) else {
        return false;
    };
    let Some(v) = (unsafe { read_val(m, val, "rin_bind_resolved: val") }) else {
        return false;
    };
    let handle = crate::TypeHandle::from_raw(ty.id, ty.env_id);
    match m.state.as_mut().expect("Rin state present") {
        RinState::Building(b) => match b.bind_val(name_str, handle, v) {
            Ok(()) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        RinState::Frozen(_) => {
            m.record_simple_error(
                "env.frozen",
                RinErrorKind::Runtime,
                Span::default(),
                "cannot bind global variable on frozen Rin",
            );
            false
        }
    }
}

/// 凍結後の環境で、事前解決済みの [`RinResolvedBinding`] を与えてソースを評価する。
///
/// # Safety
/// `machine`・`source` は有効なポインタ、`bindings` は `binding_count` 個の有効な要素を指すこと。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_eval_with_resolved(
    machine: *mut Rin,
    source: *const c_char,
    bindings: *const RinResolvedBinding,
    binding_count: usize,
) -> RinEvalResult {
    let Some(m) = (unsafe { machine.as_mut() }) else {
        return RinEvalResult::failure();
    };
    m.clear_error();
    let Some(src) = (unsafe { read_c_str(m, source, "rin_eval_with_resolved: source") }) else {
        return RinEvalResult::failure();
    };
    if binding_count > 0 && bindings.is_null() {
        m.record_host_error("rin_eval_with_resolved: bindings is NULL");
        return RinEvalResult::failure();
    }
    if matches!(m.state_ref(), RinState::Building(_)) {
        m.record_simple_error(
            "env.phase",
            RinErrorKind::Runtime,
            Span::default(),
            "rin_eval_with_resolved requires a frozen Rin; call rin_freeze first or use rin_build",
        );
        return RinEvalResult::failure();
    }

    type ResolvedEntry<'b> = (&'b str, crate::TypeHandle, Val);
    let mut stack_buf: [ResolvedEntry<'_>; STACK_FFI_ARGS] =
        [("", crate::TypeHandle::from_raw(0, 0), Val::void()); STACK_FFI_ARGS];
    let mut heap_ptr: Option<(NonNull<ResolvedEntry<'_>>, Layout)> = None;
    let slice_mut: &mut [ResolvedEntry<'_>] = if binding_count <= STACK_FFI_ARGS {
        &mut stack_buf[..binding_count]
    } else {
        let Some(layout) = Layout::array::<ResolvedEntry<'_>>(binding_count).ok() else {
            m.record_simple_error(
                "eval.limits",
                RinErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return RinEvalResult::failure();
        };
        let mut calloc = m.alloc;
        let Some(ptr) = calloc.alloc(layout) else {
            m.record_simple_error(
                "eval.limits",
                RinErrorKind::TooMuchMemory,
                Span::default(),
                "memory limit exceeded",
            );
            return RinEvalResult::failure();
        };
        let typed = ptr.cast::<ResolvedEntry<'_>>();
        heap_ptr = Some((typed, layout));
        unsafe { std::slice::from_raw_parts_mut(typed.as_ptr(), binding_count) }
    };

    let mut filled = 0usize;
    for i in 0..binding_count {
        let entry = unsafe { &*bindings.add(i) };
        let Some(bname) = (unsafe { read_c_str(m, entry.name, "rin_eval_with_resolved: name") })
        else {
            for &(_, _, v) in &slice_mut[..filled] {
                m.state_mut().release_val(v);
            }
            if let Some((p, layout)) = heap_ptr {
                let mut calloc = m.alloc;
                unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
            }
            return RinEvalResult::failure();
        };
        let Some(v) = (unsafe { read_val(m, entry.value, "rin_eval_with_resolved: value") }) else {
            for &(_, _, v) in &slice_mut[..filled] {
                m.state_mut().release_val(v);
            }
            if let Some((p, layout)) = heap_ptr {
                let mut calloc = m.alloc;
                unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
            }
            return RinEvalResult::failure();
        };
        m.state_mut().retain_val(v);
        slice_mut[i] = (
            bname,
            crate::TypeHandle::from_raw(entry.ty.id, entry.ty.env_id),
            v,
        );
        filled += 1;
    }

    let ok = match m.state.as_mut().expect("Rin state present") {
        RinState::Frozen(f) => match f.eval_with_vals(src, slice_mut) {
            Ok(_) => true,
            Err(e) => {
                m.last_error.set_from_ref(e);
                m.has_error = true;
                false
            }
        },
        RinState::Building(_) => unreachable!(),
    };

    for &(_, _, v) in &slice_mut[..filled] {
        m.state_mut().release_val(v);
    }
    if let Some((p, layout)) = heap_ptr {
        let mut calloc = m.alloc;
        unsafe { calloc.dealloc(p.cast::<u8>(), layout) };
    }

    if ok {
        m.finish_eval_result()
    } else {
        RinEvalResult::failure()
    }
}

/// 大域関数 `defn` の仮引数個数を `*out_arity` へ書き込む。見つからない・NULL の場合は `false`。
///
/// # Safety
/// `machine`・`fn_name`・`out_arity` は有効なポインタであること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_fn_arity(
    machine: *const Rin,
    fn_name: *const c_char,
    out_arity: *mut usize,
) -> bool {
    let Some(m) = (unsafe { machine.as_ref() }) else {
        return false;
    };
    if fn_name.is_null() || out_arity.is_null() {
        return false;
    }
    let Ok(name) = (unsafe { CStr::from_ptr(fn_name) }).to_str() else {
        return false;
    };
    let sig = match m.state_ref() {
        RinState::Building(b) => b.fn_sig(name),
        RinState::Frozen(f) => f.fn_sig(name),
    };
    let Some(sig) = sig else {
        return false;
    };
    unsafe { *out_arity = sig.param_count() };
    true
}

/// モジュール公開関数 `defn` の仮引数個数を `*out_arity` へ書き込む。見つからない・NULL の場合は `false`。
///
/// # Safety
/// `machine`・`mod_name`・`fn_name`・`out_arity` は有効なポインタであること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_fn_arity_in(
    machine: *const Rin,
    mod_name: *const c_char,
    fn_name: *const c_char,
    out_arity: *mut usize,
) -> bool {
    let Some(m) = (unsafe { machine.as_ref() }) else {
        return false;
    };
    if mod_name.is_null() || fn_name.is_null() || out_arity.is_null() {
        return false;
    }
    let Ok(mname) = (unsafe { CStr::from_ptr(mod_name) }).to_str() else {
        return false;
    };
    let Ok(fname) = (unsafe { CStr::from_ptr(fn_name) }).to_str() else {
        return false;
    };
    let sig = match m.state_ref() {
        RinState::Building(b) => b.fn_sig_in(mname, fname),
        RinState::Frozen(f) => f.fn_sig_in(mname, fname),
    };
    let Some(sig) = sig else {
        return false;
    };
    unsafe { *out_arity = sig.param_count() };
    true
}

struct FixedBufWriter {
    ptr: *mut u8,
    cap: usize,
    written: usize,
    total: usize,
}

impl std::fmt::Write for FixedBufWriter {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let bytes = s.as_bytes();
        self.total = self.total.saturating_add(bytes.len());
        if !self.ptr.is_null() && self.cap > 0 {
            let max_content = self.cap - 1;
            let avail = max_content.saturating_sub(self.written);
            if avail > 0 {
                let mut take = bytes.len().min(avail);
                while take > 0 && !s.is_char_boundary(take) {
                    take -= 1;
                }
                if take > 0 {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            bytes.as_ptr(),
                            self.ptr.add(self.written),
                            take,
                        );
                    }
                    self.written += take;
                }
            }
        }
        Ok(())
    }
}

fn format_sig_to_c_buf(sig: crate::FnSigRef<'_>, out_buf: *mut c_char, out_cap: usize) -> usize {
    use std::fmt::Write as _;
    let mut w = FixedBufWriter {
        ptr: out_buf.cast::<u8>(),
        cap: out_cap,
        written: 0,
        total: 0,
    };
    let _ = write!(&mut w, "{sig}");
    if !out_buf.is_null() && out_cap > 0 {
        unsafe { *out_buf.add(w.written) = 0 };
    }
    w.total
}

/// 大域関数 `defn` のシグネチャを `out_buf`（最大 `out_cap` バイト、NUL 終端）へ書き出し、必要なバイト数（NUL 除く）を返す。
/// 関数が存在しない場合は `0` を返す。
///
/// # Safety
/// `machine`・`fn_name` は有効なポインタ、`out_cap > 0` のときは `out_buf` が有効であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_fn_sig_format(
    machine: *const Rin,
    fn_name: *const c_char,
    out_buf: *mut c_char,
    out_cap: usize,
) -> usize {
    let Some(m) = (unsafe { machine.as_ref() }) else {
        return 0;
    };
    if fn_name.is_null() {
        return 0;
    }
    let Ok(name) = (unsafe { CStr::from_ptr(fn_name) }).to_str() else {
        return 0;
    };
    let sig = match m.state_ref() {
        RinState::Building(b) => b.fn_sig(name),
        RinState::Frozen(f) => f.fn_sig(name),
    };
    match sig {
        Some(s) => format_sig_to_c_buf(s, out_buf, out_cap),
        None => 0,
    }
}

/// モジュール公開関数 `defn` のシグネチャを `out_buf`（最大 `out_cap` バイト、NUL 終端）へ書き出し、必要なバイト数（NUL 除く）を返す。
/// 関数が存在しない場合は `0` を返す。
///
/// # Safety
/// `machine`・`mod_name`・`fn_name` は有効なポインタ、`out_cap > 0` のときは `out_buf` が有効であること。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_fn_sig_format_in(
    machine: *const Rin,
    mod_name: *const c_char,
    fn_name: *const c_char,
    out_buf: *mut c_char,
    out_cap: usize,
) -> usize {
    let Some(m) = (unsafe { machine.as_ref() }) else {
        return 0;
    };
    if mod_name.is_null() || fn_name.is_null() {
        return 0;
    }
    let Ok(mname) = (unsafe { CStr::from_ptr(mod_name) }).to_str() else {
        return 0;
    };
    let Ok(fname) = (unsafe { CStr::from_ptr(fn_name) }).to_str() else {
        return 0;
    };
    let sig = match m.state_ref() {
        RinState::Building(b) => b.fn_sig_in(mname, fname),
        RinState::Frozen(f) => f.fn_sig_in(mname, fname),
    };
    match sig {
        Some(s) => format_sig_to_c_buf(s, out_buf, out_cap),
        None => 0,
    }
}

/// 直近の対象操作のエラーを借用する。エラーなし、または NULL machine なら NULL。
/// 次の rin_build / rin_eval / rin_eval_with / rin_load / rin_call / rin_call_in / rin_bind / rin_freeze / rin_set_math_fma、または rin_free まで有効。
/// この取得関数はエラーを無効化しない。ホストは解放しない。
///
/// # Safety
/// `machine` は NULL または有効な Rin を指し、対象操作と同時に呼ばないこと。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rin_last_error(machine: *const Rin) -> *const RinError {
    // SAFETY: 呼び出し側が NULL または有効なポインタを保証する。
    let Some(m) = (unsafe { machine.as_ref() }) else {
        return std::ptr::null();
    };
    if m.has_error {
        &m.last_error.view
    } else {
        std::ptr::null()
    }
}
