//! 同じ評価を別プロセス・別ビルドで実行し、値と診断の全出力を比較する。
use super::*;
use rin::{Builder, HeapAllocator};
use std::fmt::Write;
use std::process::Command;

fn record(out: &mut String, label: &str, source: &str, result: Result<Value, Error>) {
    match result {
        Ok(value) => writeln!(out, "{label}: value {value:?} display={value}").unwrap(),
        Err(error) => writeln!(
            out,
            "{label}: error {error:?} display={text:?} render={render:?}",
            text = error.to_string(),
            render = error.render(source),
        )
        .unwrap(),
    }
}

fn snapshot() -> String {
    let mut out = String::new();
    let docs = docs_dir();
    let index = parse_index_counts(&fs::read_to_string(docs.join("spec-cases.md")).unwrap());
    let mut count = 0;
    for (path, expected) in index.values() {
        let cases = parse_spec_cases_file(&fs::read_to_string(docs.join(path)).unwrap(), path);
        assert_eq!(cases.len(), *expected);
        for case in cases {
            record(&mut out, &case.id, &case.source, evaluate_case(&case));
            count += 1;
        }
    }
    assert!(count > 0);
    writeln!(out, "spec-cases={count}").unwrap();

    let nested = format!("{}(+ x 2){}", "(let (x 1) ".repeat(12), ")".repeat(12));
    let programs = [
        "(+ 1 2)",
        "(defn sq (x) (* x x)) (sq 7)",
        "(let (x 3) (if true (+ x 2) (/ 1 0)))",
        "(let (x 1) (and true (> x 0)) (or false (< x 0)) (match x (1 (+ x 2)) (_ 0)))",
        "(let (x 4) ((fn (y) (+ x y)) 3))",
        "(map (fn (x) (+ x 1)) (range 0 5))",
        "(fold (fn (a x) (+ a x)) 0 [1 2 3 4])",
        "(defn sum ((n i64) (a i64)) -> i64 (if (== n 0) a (sum (- n 1) (+ a n)))) (sum 5 0)",
        "(defn inner ((x i64)) -> i64 (/ x 0)) (defn outer ((x i64)) -> i64 (+ 1 (inner x))) (outer 5)",
        "(sort_by (fn (x) (- 0 x)) [1 4 2 3])",
        "(let (answer 1) anwser)",
        "(reverse [1 2 3])",
        "(defschema S (struct S (a (list i64)) (b i64))) (with S { a: [1] b: 2 } { a: [3] })",
        "(unique [1 2 1 3 2])",
        r#"(sort ["あいう" "あいあ" "a"])"#,
        "(flat_map (fn (x) (range 0 x)) [1 2 3])",
        "(defschema LongName (struct LongName (field (list i64)))) (== LongName { field: [1 2] } LongName { field: [1 2] })",
        "(== (tuple some([1 2]) 3) (tuple some([1 2]) 4))",
        &nested,
    ];
    for (index, source) in programs.iter().enumerate() {
        for steps in 0..=128 {
            let result = eval_with(
                source,
                &[],
                Limits {
                    steps,
                    ..Limits::default()
                },
            );
            if steps == 128 {
                assert!(
                    !matches!(&result, Err(e) if e.kind == ErrorKind::TooManySteps),
                    "{source}"
                );
            }
            record(&mut out, &format!("steps/{index}/{steps}"), source, result);
        }
    }
    for depth in 0..=8 {
        let source = "(defn f ((n i64)) -> i64 (+ 1 (f n))) (f 0)";
        record(
            &mut out,
            &format!("depth/{depth}"),
            source,
            eval_with(
                source,
                &[],
                Limits {
                    depth,
                    ..Limits::default()
                },
            ),
        );
    }
    for memory_bytes in (0..=512).step_by(16) {
        let source = "(list->array (range 0 8))";
        record(
            &mut out,
            &format!("memory/{memory_bytes}"),
            source,
            eval_with(
                source,
                &[],
                Limits {
                    memory_bytes,
                    ..Limits::default()
                },
            ),
        );
    }
    // NaNの符号・ペイロードは保証対象外。表示はNaNとして比較する。
    let numeric = "[(+ 0.1 0.2), -0.0, (/ 1.0 0.0), (/ 0.0 0.0), (sqrt 4.0), (floor 1.5), (ceil -1.5), (abs -2.0)]";
    record(&mut out, "numeric", numeric, eval(numeric));
    trig_modes(&mut out);
    stateful(&mut out);
    out
}

fn trig_modes(out: &mut String) {
    let source = "[(sin 0.0), (sin -0.0), (cos 0.0), (sin 0.5), (cos 0.5), (sin (/ pi 6.0)), (cos (/ pi 4.0)), (sin (/ pi 2.0)), (cos pi), (sin 100.0), (cos 100.0), (sin 1.0e20), (cos 1.0e20), (sin 1.0e300), (cos 1.0e300), (sin inf), (cos nan)]";
    for (label, mode) in [
        ("trig-strict", rin::MathFma::Strict),
        ("trig-fused", rin::MathFma::Fused),
    ] {
        let mut alloc = HeapAllocator;
        let mut machine = Builder::new(Limits::default(), &mut alloc);
        machine.set_math_fma(mode);
        let result = machine.eval(source);
        if let Ok(Value::List(list)) = &result {
            let bits: Vec<String> = list
                .iter()
                .map(|v| match v {
                    Value::Float(x) if x.is_nan() => "NaN".to_string(),
                    Value::Float(x) => format!("0x{:016x}", x.to_bits()),
                    other => format!("{other:?}"),
                })
                .collect();
            writeln!(out, "{label}-bits: [{}]", bits.join(", ")).unwrap();
        }
        record(out, label, source, result);
    }
}

fn stateful(out: &mut String) {
    let mut alloc = HeapAllocator;
    let mut machine = Builder::new(Limits::default(), &mut alloc);
    for name in ["alpho", "alpha", "alphi"] {
        machine.bind(name, "i64", &Value::Int(1)).unwrap();
    }
    for (name, source) in [
        (
            "base",
            "(export zeta alpha) (def zeta 2) (defn alpha ((x i64)) -> i64 (/ x 0)) (def hidden 9)",
        ),
        (
            "client",
            "(import base) (defn call ((x i64)) -> i64 (+ 1 (base.alpha x)))",
        ),
    ] {
        machine.load_module(name, source).unwrap();
    }
    for source in [
        "alphx",
        "(import base) base.unknown",
        "(import baes)",
        "Foo { z: 3 a: 1 m: 2 }",
        "Foo { z: 3 a: 1 m: 2 }.unknown",
    ] {
        record(out, source, source, machine.eval(source));
    }
    {
        let mut alloc2 = HeapAllocator;
        let mut b2 = Builder::new(Limits::default(), &mut alloc2);
        b2.load_module(
            "base",
            "(export zeta alpha) (def zeta 2) (defn alpha ((x i64)) -> i64 (/ x 0)) (def hidden 9)",
        )
        .unwrap();
        b2.load_module(
            "client",
            "(import base) (defn call ((x i64)) -> i64 (+ 1 (base.alpha x)))",
        )
        .unwrap();
        let mut frozen = b2.freeze().unwrap();
        record(
            out,
            "host-call",
            "",
            frozen.call_in("client", "call", &[Value::Int(2)]),
        );
        record(
            out,
            "frozen-module-access",
            "base.zeta",
            frozen.eval("base.zeta"),
        );
        record(
            out,
            "frozen-def-rejected",
            "(def x 1)",
            frozen.eval("(def x 1)"),
        );
    }
    record(
        out,
        "reload-rejected",
        "",
        machine
            .load_module("base", "(export zeta) (def zeta 7)")
            .map(|()| Value::Void),
    );
    let mut machine = machine.freeze().unwrap();
    record(
        out,
        "frozen-call",
        "",
        machine.call_in("base", "alpha", &[]),
    );
    // 文字列のインターン表がHashMapへ切り替わった後も、出力順が安定すること。
    let strings = format!(
        "[{}]",
        (0..40)
            .rev()
            .map(|i| format!("\"s{i}\""))
            .collect::<Vec<_>>()
            .join(" ")
    );
    record(out, "interned-strings", &strings, machine.eval(&strings));
}

#[test]
#[ignore = "check-determinism.shと別プロセス比較から出力先を指定して実行する"]
fn write_snapshot() {
    let path = std::env::var_os("RIN_DETERMINISM_OUTPUT").expect("snapshot output path");
    fs::write(path, snapshot()).unwrap();
}

#[test]
fn separate_processes_produce_identical_results() {
    let directory = std::env::temp_dir().join(format!("rin-determinism-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    let mut previous = None;
    for n in 0..3 {
        let path = directory.join(format!("{n}.txt"));
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "determinism::write_snapshot", "--ignored"])
            .env("RIN_DETERMINISM_OUTPUT", &path)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let bytes = fs::read(path).unwrap();
        assert!(!bytes.is_empty());
        if let Some(previous) = &previous {
            assert_eq!(&bytes, previous, "process {n}");
        }
        previous = Some(bytes);
    }
}
