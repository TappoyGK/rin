//! C FFI の評価・モジュール読み込み・関数呼び出し・エラー取得 API。

use std::alloc::Layout;
use std::ffi::{CStr, c_char};
use std::ptr::NonNull;

use crate::alloc::Allocator;
use crate::error::Span;
use crate::value_ref::Val;

use super::{
    Rin, RinBinding, RinError, RinErrorKind, RinEvalResult, RinState, RinValue, begin_operation,
};

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
