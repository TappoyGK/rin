//! 決定的な `sin`・`cos` の共有テスト値（`docs/math-vectors/sin-cos.tsv`）、
//! `MathFma` 設定、および C FFI の設定 API を検証する。

use std::fs;
use std::path::PathBuf;

use rin::ffi::{
    RinAllocator, RinErrorKind, RinMathFma, rin_as_float, rin_bind, rin_drop, rin_eval, rin_float,
    rin_free, rin_freeze, rin_last_error, rin_limits_default, rin_math_fma, rin_new,
    rin_set_math_fma,
};
use rin::math::{self, MathFma};
use rin::{Builder, HeapAllocator, Limits, Value};

fn vectors_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("docs/math-vectors/sin-cos.tsv")
}

fn parse_hex_u64(s: &str) -> u64 {
    u64::from_str_radix(s.strip_prefix("0x").unwrap(), 16).unwrap()
}

fn assert_bits_eq(actual: f64, expected: &str, context: &str) {
    if expected == "NaN" {
        assert!(actual.is_nan(), "{context}: expected NaN, got {actual:?}");
    } else {
        let want = parse_hex_u64(expected);
        let got = actual.to_bits();
        assert_eq!(
            got, want,
            "{context}: expected 0x{want:016x}, got 0x{got:016x}"
        );
    }
}

#[test]
fn trig_matches_shared_vectors_strict_and_fused() {
    let content = fs::read_to_string(vectors_path()).unwrap();
    let mut count = 0usize;
    let mut strict_fused_diffs = 0usize;

    let mut alloc_s = HeapAllocator;
    let mut machine_s = Builder::new(Limits::default(), &mut alloc_s);
    assert_eq!(machine_s.math_fma(), MathFma::Strict);

    let mut alloc_f = HeapAllocator;
    let mut machine_f = Builder::new(Limits::default(), &mut alloc_f);
    machine_f.set_math_fma(MathFma::Fused);
    assert_eq!(machine_f.math_fma(), MathFma::Fused);

    for (lineno, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('\t').collect();
        assert_eq!(cols.len(), 5, "line {}: {line}", lineno + 1);
        let (func, in_hex, strict_hex, fused_hex, note) =
            (cols[0], cols[1], cols[2], cols[3], cols[4]);
        let x = f64::from_bits(parse_hex_u64(in_hex));
        if strict_hex != fused_hex {
            strict_fused_diffs += 1;
        }

        let (got_s, got_f) = match func {
            "sin" => (math::sin(x, MathFma::Strict), math::sin(x, MathFma::Fused)),
            "cos" => (math::cos(x, MathFma::Strict), math::cos(x, MathFma::Fused)),
            other => panic!("unknown func {other}"),
        };
        assert_bits_eq(
            got_s,
            strict_hex,
            &format!("{func}({in_hex}) strict [{note}]"),
        );
        assert_bits_eq(
            got_f,
            fused_hex,
            &format!("{func}({in_hex}) fused [{note}]"),
        );

        // Machine 経由の評価でも同じビット列を返すこと。
        let name = format!("x_{count}");
        machine_s.bind(&name, &Value::Float(x)).unwrap();
        machine_f.bind(&name, &Value::Float(x)).unwrap();
        let src = format!("({func} {name})");
        let Value::Float(eval_s) = machine_s.eval(&src).unwrap() else {
            panic!("expected float");
        };
        let Value::Float(eval_f) = machine_f.eval(&src).unwrap() else {
            panic!("expected float");
        };
        assert_bits_eq(eval_s, strict_hex, &format!("eval {src} strict [{note}]"));
        assert_bits_eq(eval_f, fused_hex, &format!("eval {src} fused [{note}]"));

        count += 1;
    }

    assert!(count >= 70, "expected >= 70 vectors, got {count}");
    assert!(
        strict_fused_diffs > 0,
        "vectors must include cases where Strict and Fused differ"
    );
}

#[test]
fn user_arithmetic_is_never_fused_even_in_fused_mode() {
    // a = 1 + 2^-52, b = 1 - 2^-52, c = -1.0
    // 非融合 (a * b) + c = 1.0 + (-1.0) = 0.0
    // FMA fma(a, b, c)   = -2^-104 != 0.0
    let a = f64::from_bits(0x3ff0_0000_0000_0001);
    let b = f64::from_bits(0x3fef_ffff_ffff_fffe);
    let c = -1.0_f64;
    assert_eq!(a.mul_add(b, c), -2.0_f64.powi(-104));

    for mode in [MathFma::Strict, MathFma::Fused] {
        let mut alloc = HeapAllocator;
        let mut machine = Builder::new(Limits::default(), &mut alloc);
        machine.set_math_fma(mode);
        machine.bind("a", &Value::Float(a)).unwrap();
        machine.bind("b", &Value::Float(b)).unwrap();
        machine.bind("c", &Value::Float(c)).unwrap();
        let Value::Float(res) = machine.eval("(+ (* a b) c)").unwrap() else {
            panic!("expected float");
        };
        assert_eq!(res.to_bits(), 0.0_f64.to_bits(), "mode={mode:?}");
    }
}

#[test]
fn c_ffi_math_fma_api() {
    let alloc = RinAllocator {
        ctx: std::ptr::null_mut(),
        alloc: None,
        dealloc: None,
    };
    let m = rin_new(rin_limits_default(), alloc);
    assert!(!m.is_null());

    unsafe {
        assert_eq!(rin_math_fma(std::ptr::null()), RinMathFma::Strict);
        assert!(!rin_set_math_fma(std::ptr::null_mut(), 1));

        assert_eq!(rin_math_fma(m), RinMathFma::Strict);
        assert!(rin_set_math_fma(m, RinMathFma::Fused as i32));
        assert_eq!(rin_math_fma(m), RinMathFma::Fused);
        assert!(rin_last_error(m).is_null());

        // 不正値は false と RIN_ERR_HOST を返す。
        assert!(!rin_set_math_fma(m, 99));
        let err = rin_last_error(m);
        assert!(!err.is_null());
        assert_eq!((*err).kind, RinErrorKind::Host);
        assert_eq!(rin_math_fma(m), RinMathFma::Fused);

        // Strict と Fused で結果が 1 bit 分かれる入力を bind して、設定が評価に効くことを確かめる。
        let input = rin_float(m, f64::from_bits(0x3fcb_a7ce_3783_fd35));
        assert!(rin_bind(m, c"x".as_ptr(), input));
        rin_drop(m, input);
        assert!(rin_freeze(m));

        let res_fused = rin_eval(m, c"(sin x)".as_ptr());
        assert!(res_fused.ok);
        assert!(rin_last_error(m).is_null());
        let mut out_fused = 0.0;
        assert!(rin_as_float(m, res_fused.value, &mut out_fused));
        assert_eq!(out_fused.to_bits(), 0x3fcb_70d9_ff96_56af);

        assert!(rin_set_math_fma(m, RinMathFma::Strict as i32));
        assert_eq!(rin_math_fma(m), RinMathFma::Strict);
        let res_strict = rin_eval(m, c"(sin x)".as_ptr());
        assert!(res_strict.ok);
        let mut out_strict = 0.0;
        assert!(rin_as_float(m, res_strict.value, &mut out_strict));
        assert_eq!(out_strict.to_bits(), 0x3fcb_70d9_ff96_56ae);

        rin_free(m);
    }
}
