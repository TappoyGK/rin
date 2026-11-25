//! 大域環境の凍結（`[env.phase]`・`[env.frozen]`・`[env.modules]`・`[env.isolation]`）と
//! C FFI の `rin_freeze` を検証する。

use std::ffi::CStr;

use rin::ffi::{
    RinAllocator, RinErrorKind, rin_as_int, rin_bind, rin_call, rin_call_in, rin_drop, rin_eval,
    rin_free, rin_freeze, rin_int, rin_last_error, rin_limits_default, rin_load, rin_new,
    rin_res_free,
};
use rin::{Builder, ErrorKind, HeapAllocator, Limits, Value};

#[test]
fn frozen_rejects_declarations_and_module_forms_before_execution() {
    let mut alloc = HeapAllocator;
    let mut builder = Builder::new(Limits::default(), &mut alloc);
    builder.bind("x", &Value::Int(10)).unwrap();
    let mut machine = builder.freeze().unwrap();

    for src in [
        "(def a 1)",
        "(defn f (n) n)",
        "(import m)",
        "(export x)",
        "(if false (def a 1) 0)",
        "(fn () (defn f () 1))",
        "(let (y 1) (import m) y)",
        "(/ 1 0) (def late 1)", // 手前のゼロ除算より前に構文エラーで止まること
    ] {
        let err = machine.eval(src).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse, "{src}");
        assert_eq!(err.rule(), "env.frozen", "{src}");
    }

    // 深い入れ子でも非再帰検査で安全に拒否する。
    let deep = format!(
        "{}(def nested 1){}",
        "(let (a 1) ".repeat(200),
        ")".repeat(200)
    );
    let err = machine.eval(&deep).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "env.frozen");
}

#[test]
fn non_top_level_declarations_are_syntax_errors_even_during_loading() {
    let mut alloc = HeapAllocator;
    let mut builder = Builder::new(Limits::default(), &mut alloc);
    for (src, rule) in [
        ("(defn g () (def y 1) y)", "expr.def"),
        ("(defn mk () (defn made () 42) 0)", "expr.defn"),
        ("(defn late () (import lib) 0)", "expr.module"),
        ("(defn exp () (export x) 0)", "expr.module"),
        ("(if false (def y 1) 0)", "expr.def"),
        ("(let (a 1) (def y 2) y)", "expr.def"),
        ("(fn () (def y 1))", "expr.def"),
    ] {
        let err = builder.eval(src).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse, "{src}");
        assert_eq!(err.rule(), rule, "{src}");
    }
    let mod_err = builder
        .load_module("mm", "(def z 0) (defn h () (def z (+ z 1)) z)")
        .unwrap_err();
    assert_eq!(mod_err.kind, ErrorKind::Parse);
    assert_eq!(mod_err.rule(), "expr.def");
}

#[test]
fn frozen_exposes_loaded_modules_without_import_and_keeps_isolation() {
    let mut alloc = HeapAllocator;
    let mut builder = Builder::new(Limits::default(), &mut alloc);
    builder
        .load_module(
            "math_mod",
            "(export add base_val) (def secret 99) (def base_val 40) (defn add (a b) (+ a b))",
        )
        .unwrap();
    builder
        .load_module("other_mod", "(defn read_unimported () math_mod.base_val)")
        .unwrap();
    let mut machine = builder.freeze().unwrap();

    // 実行段階では import なしで公開シンボルを参照できる。
    assert_eq!(
        machine.eval("(math_mod.add math_mod.base_val 2)").unwrap(),
        Value::Int(42)
    );

    // 非公開シンボルは依然として参照できない。
    let priv_err = machine.eval("math_mod.secret").unwrap_err();
    assert_eq!(priv_err.kind, ErrorKind::Runtime);
    assert_eq!(priv_err.rule(), "expr.module");

    // モジュール内の関数から、import していない別モジュールへはアクセスできない。
    let iso_err = machine
        .call_in("other_mod", "read_unimported", &[])
        .unwrap_err();
    assert_eq!(iso_err.kind, ErrorKind::Runtime);
    assert_eq!(iso_err.rule(), "expr.name");

    // 凍結時の未定義名候補には読み込み済みモジュール名も含まれる。
    let hint_err = machine.eval("math_mo").unwrap_err();
    assert_eq!(hint_err.rule(), "expr.name");
    assert!(
        hint_err.message.contains("did you mean 'math_mod'?"),
        "{}",
        hint_err.message
    );
}

#[test]
fn eval_with_temporary_bindings_do_not_mutate_or_leak() {
    let mut alloc = HeapAllocator;
    let mut builder = Builder::new(
        Limits {
            memory_bytes: 1024,
            ..Limits::default()
        },
        &mut alloc,
    );
    builder.bind("x", &Value::Int(10)).unwrap();
    builder.eval("(defn read_x () x)").unwrap();
    let mut machine = builder.freeze().unwrap();
    let baseline = machine.live_bytes();

    // 一時束縛はトップレベル評価中だけ有効で、大域 x を一時的に覆う。
    let v = machine
        .eval_with("(+ x y)", &[("x", Value::Int(100)), ("y", Value::Int(23))])
        .unwrap();
    assert_eq!(v, Value::Int(123));
    assert_eq!(machine.live_bytes(), baseline);

    // 評価後は元の環境が保たれ、一時束縛 y は消えている。
    assert_eq!(machine.eval("x").unwrap(), Value::Int(10));
    assert_eq!(machine.call("read_x", &[]).unwrap(), Value::Int(10));
    let err = machine.eval("y").unwrap_err();
    assert_eq!(err.rule(), "expr.name");
    assert_eq!(machine.live_bytes(), baseline);

    // 一時束縛が実行時エラーやメモリ超過になっても解放される。
    let err = machine
        .eval_with("(/ x 0)", &[("tmp", Value::str("hello"))])
        .unwrap_err();
    assert_eq!(err.rule(), "op.arith");
    assert_eq!(machine.live_bytes(), baseline);

    let big = Value::str("a".repeat(2048));
    let mem_err = machine
        .eval_with("x", &[("small", Value::str("ok")), ("big", big)])
        .unwrap_err();
    assert_eq!(mem_err.kind, ErrorKind::TooMuchMemory);
    assert_eq!(machine.live_bytes(), baseline);
}

#[test]
fn c_ffi_freeze_blocks_mutations_and_enables_direct_module_access() {
    let alloc = RinAllocator {
        ctx: std::ptr::null_mut(),
        alloc: None,
        dealloc: None,
    };
    let m = rin_new(rin_limits_default(), alloc);
    assert!(!m.is_null());

    unsafe {
        assert!(!rin_freeze(std::ptr::null_mut()));

        let v10 = rin_int(10);
        assert!(rin_bind(m, c"g".as_ptr(), v10));
        rin_drop(v10);

        let mut load_res = rin_load(m, c"mod_a".as_ptr(), c"(defn twice (n) (* n 2))".as_ptr());
        assert!(load_res.ok);
        rin_res_free(&mut load_res);

        let mut def_res = rin_eval(m, c"(defn add_g (n) (+ n g))".as_ptr());
        assert!(def_res.ok);
        rin_res_free(&mut def_res);

        // 凍結（2 回目も冪等に成功）
        assert!(rin_freeze(m));
        assert!(rin_last_error(m).is_null());
        assert!(rin_freeze(m));

        // 凍結後の rin_bind は env.frozen エラー
        let v20 = rin_int(20);
        assert!(!rin_bind(m, c"g".as_ptr(), v20));
        rin_drop(v20);
        let err = rin_last_error(m);
        assert!(!err.is_null());
        assert_eq!((*err).kind, RinErrorKind::Runtime);
        assert_eq!(CStr::from_ptr((*err).rule).to_str().unwrap(), "env.frozen");

        // 凍結後の rin_load も env.frozen エラー
        let mut load2 = rin_load(m, c"mod_b".as_ptr(), c"(def z 1)".as_ptr());
        assert!(!load2.ok);
        let err = rin_last_error(m);
        assert!(!err.is_null());
        assert_eq!((*err).kind, RinErrorKind::Runtime);
        assert_eq!(CStr::from_ptr((*err).rule).to_str().unwrap(), "env.frozen");
        rin_res_free(&mut load2);

        // 凍結後の def を含む eval は env.frozen の構文エラー
        let mut bad_eval = rin_eval(m, c"(def z 1)".as_ptr());
        assert!(!bad_eval.ok);
        let err = rin_last_error(m);
        assert!(!err.is_null());
        assert_eq!((*err).kind, RinErrorKind::Parse);
        assert_eq!(CStr::from_ptr((*err).rule).to_str().unwrap(), "env.frozen");
        rin_res_free(&mut bad_eval);

        // 凍結後は import なしでモジュールを参照でき、call / call_in も動く
        let mut ok_eval = rin_eval(m, c"(mod_a.twice g)".as_ptr());
        assert!(ok_eval.ok);
        let mut out = 0;
        assert!(rin_as_int(ok_eval.value, &mut out));
        assert_eq!(out, 20);
        rin_res_free(&mut ok_eval);

        let arg = rin_int(5);
        let args = [arg as *const _];
        let mut call_res = rin_call(m, c"add_g".as_ptr(), args.as_ptr(), 1);
        assert!(call_res.ok);
        assert!(rin_as_int(call_res.value, &mut out));
        assert_eq!(out, 15);
        rin_res_free(&mut call_res);

        let mut call_in_res =
            rin_call_in(m, c"mod_a".as_ptr(), c"twice".as_ptr(), args.as_ptr(), 1);
        assert!(call_in_res.ok);
        assert!(rin_as_int(call_in_res.value, &mut out));
        assert_eq!(out, 10);
        rin_res_free(&mut call_in_res);
        rin_drop(arg);

        rin_free(m);
    }
}
