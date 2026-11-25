//! FFI のエラー記録、借用寿命、早期リターンを公開 API から検証する。
use rin::ffi::*;
use std::ffi::{CStr, CString};
use std::ptr::{null, null_mut};

struct Machine(*mut Rin);
impl Machine {
    fn new(limits: RinLimits) -> Self {
        Self(rin_new(
            limits,
            RinAllocator {
                ctx: null_mut(),
                alloc: None,
                dealloc: None,
            },
        ))
    }
}
impl Drop for Machine {
    fn drop(&mut self) {
        unsafe { rin_free(self.0) };
    }
}

// 戻り値の所有物を処理し、エラーは machine から別途読む。
unsafe fn finish(mut result: RinEvalResult, ok: bool) {
    assert_eq!(result.ok, ok);
    if !ok {
        assert!(result.value.is_null());
    }
    unsafe { rin_res_free(&mut result) };
    assert!(result.value.is_null());
}
unsafe fn check_error(m: *mut Rin, kind: RinErrorKind) {
    unsafe {
        let p = rin_last_error(m);
        assert!(!p.is_null());
        let e = &*p;
        assert_eq!(e.kind, kind);
        assert!(!e.rule.is_null());
        assert!(!CStr::from_ptr(e.rule).to_bytes().is_empty());
        assert!(!e.message.is_null());
        assert!(!e.text.is_null());
        assert!(!CStr::from_ptr(e.text).to_str().unwrap().is_empty());
        if kind == RinErrorKind::Host {
            assert_eq!((e.line, e.col), (0, 0));
            assert!(e.module.is_null() && e.backtrace.is_null());
            assert_eq!(CStr::from_ptr(e.rule), c"host.argument");
            assert_eq!(
                CStr::from_ptr(e.text).to_str().unwrap(),
                format!(
                    "{} [host.argument]",
                    CStr::from_ptr(e.message).to_str().unwrap()
                )
            );
        }
    }
}

#[test]
fn kinds_modules_and_borrowed_lifetime() {
    unsafe {
        let m = Machine::new(rin_limits_default());
        assert!(rin_last_error(m.0).is_null());
        for (source, kind) in [
            (c"\"", RinErrorKind::Lex),
            (c"(", RinErrorKind::Parse),
            (c"(/ 1 0)", RinErrorKind::Runtime),
        ] {
            finish(rin_load(m.0, c"mod".as_ptr(), source.as_ptr()), false);
            check_error(m.0, kind);
            let e = &*rin_last_error(m.0);
            assert_eq!(CStr::from_ptr(e.module), c"mod");
            assert!(e.line > 0 && e.col > 0);
        }
        let mut loaded = rin_load(
            m.0,
            c"mod".as_ptr(),
            c"(defn fail () (/ 1 0)) (defn good () 42)".as_ptr(),
        );
        assert!(loaded.ok && loaded.value.is_null());
        rin_res_free(&mut loaded);
        assert!(rin_last_error(m.0).is_null());
        let mut result = rin_call_in(m.0, c"mod".as_ptr(), c"fail".as_ptr(), null(), 0);
        assert!(!result.ok);
        let p = rin_last_error(m.0);
        check_error(m.0, RinErrorKind::Runtime);
        let text = CStr::from_ptr((*p).text).to_owned();
        let rule = CStr::from_ptr((*p).rule).to_owned();
        assert_eq!(rule.as_c_str(), c"op.arith");
        assert!(!(*p).backtrace.is_null());
        assert_eq!(CStr::from_ptr((*p).module), c"mod");
        assert_eq!(p, rin_last_error(m.0));
        rin_res_free(&mut result);
        assert_eq!(p, rin_last_error(m.0));
        assert_eq!(text.as_c_str(), CStr::from_ptr((*p).text));
        assert_eq!(rule.as_c_str(), CStr::from_ptr((*p).rule));
        finish(
            rin_call_in(m.0, c"mod".as_ptr(), c"good".as_ptr(), null(), 0),
            true,
        );
        assert!(rin_last_error(m.0).is_null());
        finish(rin_build(m.0, c"(/ 1 0)".as_ptr()), false);
        assert!((*rin_last_error(m.0)).module.is_null());
        finish(
            rin_build(m.0, c"(defn fail () (/ 1 0)) (defn good () 42)".as_ptr()),
            true,
        );
        assert!(rin_last_error(m.0).is_null());
        finish(rin_call(m.0, c"fail".as_ptr(), null(), 0), false);
        assert!((*rin_last_error(m.0)).module.is_null());
        finish(rin_call(m.0, c"good".as_ptr(), null(), 0), true);
        assert!(rin_last_error(m.0).is_null());
    }
}

#[test]
fn resource_errors_and_bind_failures_preserve_kind() {
    unsafe {
        for (limits, source, kind) in [
            (
                RinLimits {
                    steps: 0,
                    ..rin_limits_default()
                },
                c"1",
                RinErrorKind::TooManySteps,
            ),
            (
                RinLimits {
                    depth: 2,
                    ..rin_limits_default()
                },
                c"(defn f () (+ 1 (f))) (f)",
                RinErrorKind::TooDeep,
            ),
            (
                RinLimits {
                    memory_bytes: 0,
                    ..rin_limits_default()
                },
                c"[1 2 3]",
                RinErrorKind::TooMuchMemory,
            ),
        ] {
            let m = Machine::new(limits);
            finish(rin_build(m.0, source.as_ptr()), false);
            check_error(m.0, kind);
            assert_eq!(CStr::from_ptr((*rin_last_error(m.0)).rule), c"eval.limits");
        }
        let m = Machine::new(RinLimits {
            memory_bytes: 0,
            ..rin_limits_default()
        });
        let v = rin_str(c"value".as_ptr());
        assert!(!rin_bind(m.0, c"x".as_ptr(), v));
        check_error(m.0, RinErrorKind::TooMuchMemory);
        rin_drop(v);
        let m = Machine::new(rin_limits_default());
        let v = RinValue(rin::Value::Builtin("unknown\0builtin"));
        assert!(!rin_bind(m.0, c"x".as_ptr(), &v));
        check_error(m.0, RinErrorKind::Runtime);
        assert_eq!(CStr::from_ptr((*rin_last_error(m.0)).rule), c"host.value");
        for s in [(*rin_last_error(m.0)).message, (*rin_last_error(m.0)).text] {
            assert!(
                CStr::from_ptr(s)
                    .to_str()
                    .unwrap()
                    .contains("unknown\\0builtin")
            );
        }
    }
}

#[test]
fn all_host_argument_paths_replace_previous_error_and_success_clears_it() {
    unsafe {
        let m = Machine::new(rin_limits_default());
        let bad = [0xff_u8, 0];
        let bad = bad.as_ptr().cast();
        let v = rin_int(1);
        let missing = [null()];
        let missing_bind_name = [RinBinding {
            name: null(),
            value: v,
        }];
        let bad_bind_name = [RinBinding {
            name: bad,
            value: v,
        }];
        let missing_bind_val = [RinBinding {
            name: c"x".as_ptr(),
            value: null(),
        }];
        // 各経路の前にモジュール内のエラーを記録し、残存していないことを確かめる。
        let cases: Vec<Box<dyn Fn() -> bool>> = vec![
            Box::new(|| rin_build(m.0, null()).ok),
            Box::new(|| rin_build(m.0, bad).ok),
            Box::new(|| rin_eval(m.0, null()).ok),
            Box::new(|| rin_eval(m.0, bad).ok),
            Box::new(|| rin_eval_with(m.0, null(), null(), 0).ok),
            Box::new(|| rin_eval_with(m.0, bad, null(), 0).ok),
            Box::new(|| rin_eval_with(m.0, c"1".as_ptr(), null(), 1).ok),
            Box::new(|| rin_eval_with(m.0, c"1".as_ptr(), missing_bind_name.as_ptr(), 1).ok),
            Box::new(|| rin_eval_with(m.0, c"1".as_ptr(), bad_bind_name.as_ptr(), 1).ok),
            Box::new(|| rin_eval_with(m.0, c"1".as_ptr(), missing_bind_val.as_ptr(), 1).ok),
            Box::new(|| rin_load(m.0, null(), c"1".as_ptr()).ok),
            Box::new(|| rin_load(m.0, c"m".as_ptr(), null()).ok),
            Box::new(|| rin_load(m.0, bad, c"1".as_ptr()).ok),
            Box::new(|| rin_load(m.0, c"m".as_ptr(), bad).ok),
            Box::new(|| rin_call(m.0, null(), null(), 0).ok),
            Box::new(|| rin_call(m.0, bad, null(), 0).ok),
            Box::new(|| rin_call(m.0, c"f".as_ptr(), null(), 1).ok),
            Box::new(|| rin_call(m.0, c"f".as_ptr(), missing.as_ptr(), 1).ok),
            Box::new(|| rin_call_in(m.0, null(), c"f".as_ptr(), null(), 0).ok),
            Box::new(|| rin_call_in(m.0, c"m".as_ptr(), null(), null(), 0).ok),
            Box::new(|| rin_call_in(m.0, bad, c"f".as_ptr(), null(), 0).ok),
            Box::new(|| rin_call_in(m.0, c"m".as_ptr(), bad, null(), 0).ok),
            Box::new(|| rin_call_in(m.0, c"m".as_ptr(), c"f".as_ptr(), null(), 1).ok),
            Box::new(|| rin_call_in(m.0, c"m".as_ptr(), c"f".as_ptr(), missing.as_ptr(), 1).ok),
            Box::new(|| rin_bind(m.0, null(), v)),
            Box::new(|| rin_bind(m.0, bad, v)),
            Box::new(|| rin_bind(m.0, c"x".as_ptr(), null())),
        ];
        for case in cases {
            finish(
                rin_load(m.0, c"previous".as_ptr(), c"(/ 1 0)".as_ptr()),
                false,
            );
            assert!(!case());
            check_error(m.0, RinErrorKind::Host);
            assert!(rin_bind(m.0, c"x".as_ptr(), v));
            assert!(rin_last_error(m.0).is_null());
        }
        rin_drop(v);
        assert!(rin_last_error(null()).is_null());
        finish(rin_build(null_mut(), null()), false);
        finish(rin_eval(null_mut(), null()), false);
        finish(rin_eval_with(null_mut(), null(), null(), 0), false);
        finish(rin_load(null_mut(), null(), null()), false);
        finish(rin_call(null_mut(), null(), null(), 0), false);
        finish(rin_call_in(null_mut(), null(), null(), null(), 0), false);
        assert!(!rin_bind(null_mut(), null(), null()));
    }
}

#[test]
fn ffi_text_matches_rust_display() {
    let source = "(defn f () (/ 1 0)) (f)";
    let expected = rin::eval(source).unwrap_err();
    let source = CString::new(source).unwrap();
    unsafe {
        let m = Machine::new(rin_limits_default());
        finish(rin_build(m.0, source.as_ptr()), false);
        let e = &*rin_last_error(m.0);
        assert_eq!(CStr::from_ptr(e.rule).to_str().unwrap(), expected.rule());
        assert_eq!(
            CStr::from_ptr(e.text).to_str().unwrap(),
            expected.to_string()
        );
        assert_eq!(
            CStr::from_ptr(e.backtrace).to_str().unwrap(),
            expected.format_backtrace()
        );
        assert_eq!(
            CStr::from_ptr(e.message).to_str().unwrap(),
            expected.message
        );
    }
}

#[test]
fn hints_are_in_ffi_message_and_text_with_original_rule() {
    for (source, expected_message, rule) in [
        (
            "(defn fail (damage) damge) (fail 1)",
            "unknown name 'damge'; did you mean 'damage'?",
            "expr.name",
        ),
        (
            "Point{z: 1, a: 2}.bad",
            "Point has no field 'bad'; fields: a, z",
            "expr.field",
        ),
    ] {
        let expected = rin::eval(source).unwrap_err();
        assert_eq!(expected.message, expected_message);
        let source = CString::new(source).unwrap();
        unsafe {
            let m = Machine::new(rin_limits_default());
            finish(rin_build(m.0, source.as_ptr()), false);
            let e = &*rin_last_error(m.0);
            assert_eq!(
                CStr::from_ptr(e.message).to_str().unwrap(),
                expected_message
            );
            assert_eq!(CStr::from_ptr(e.rule).to_str().unwrap(), rule);
            assert_eq!(
                CStr::from_ptr(e.text).to_str().unwrap(),
                expected.to_string()
            );
        }
    }
}
