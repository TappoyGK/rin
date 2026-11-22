use crate::{ErrorKind, HeapAllocator, Limits, Machine, Value, eval, eval_with};
use std::collections::BTreeMap;

#[test]
fn arithmetic_and_comptime_sine_table() {
    let src = r#"
        (map (fn (i) (sin (/ (* 2.0 pi (as_f64 i)) 4.0))) (range 0 4))
    "#;
    let Value::List(ref xs) = eval(src).unwrap() else {
        panic!("expected list");
    };
    assert_eq!(xs.len(), 4);
    let Some(Value::Float(v1)) = xs.get(1) else {
        panic!("expected float");
    };
    assert!((v1 - 1.0).abs() < 1e-12);
}

#[test]
fn no_implicit_int_float_mix() {
    let err = eval("(+ 1 1.0)").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
}

#[test]
fn struct_field_index_with_and_match() {
    let mut foe = BTreeMap::new();
    foe.insert("hp".into(), Value::Int(20));
    foe.insert("element".into(), Value::variant("Element", "Frost", None));

    let mut battle = BTreeMap::new();
    battle.insert(
        "foes".into(),
        Value::list(vec![Value::structure("Foe", foe)]),
    );

    let src = r#"
        (defn calc (b target)
          (match b.foes[target].element
            (Element.Frost [(with b.foes[target] { hp: (- b.foes[target].hp 9) })])
            (Element.Fire  [(with b.foes[target] { hp: (- b.foes[target].hp 3) })])
            (_             [(with b.foes[target] { hp: (- b.foes[target].hp 6) })])))
        (calc battle 0)
    "#;
    let val = eval_with(
        src,
        &[("battle", Value::structure("BattleState", battle))],
        Limits::default(),
    )
    .unwrap();

    let Value::List(ref xs) = val else {
        panic!("expected list");
    };
    let Some(Value::Struct { fields, .. }) = xs.get(0) else {
        panic!("expected struct");
    };
    assert_eq!(fields.get("hp"), Some(&Value::Int(11)));
}

#[test]
fn nested_upvalue_closures_and_shadowing() {
    let src = r#"
        (let (a 10)
          (let (f1 (fn (b)
                     (let (c 5)
                       (fn (d) (+ a b c d)))))
            (let (a 999)
              ((f1 20) 7))))
    "#;
    assert_eq!(eval(src).unwrap(), Value::Int(42));
}

#[test]
fn edge_cases_short_circuit_overflow_when_empty_match() {
    assert_eq!(eval("").unwrap(), Value::Void);
    assert_eq!(eval("(when false (/ 1 0))").unwrap(), Value::Void);
    assert_eq!(
        eval("(and false (== (/ 1 0) 0))").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(eval("(or true (== (/ 1 0) 0))").unwrap(), Value::Bool(true));

    let overflow = eval("(+ 9223372036854775807 1)").unwrap_err();
    assert_eq!(overflow.kind, ErrorKind::Runtime);
    assert_eq!(overflow.message, "i64 overflow");

    let sub_overflow = eval("(- -9223372036854775808 1)").unwrap_err();
    assert_eq!(sub_overflow.kind, ErrorKind::Runtime);
    assert_eq!(sub_overflow.message, "i64 overflow");

    let mul_overflow = eval("(* -9223372036854775808 -1)").unwrap_err();
    assert_eq!(mul_overflow.kind, ErrorKind::Runtime);
    assert_eq!(mul_overflow.message, "i64 overflow");

    let regular_overflow = eval("(+ 9223372036854775807 1 0)").unwrap_err();
    assert_eq!(regular_overflow.kind, ErrorKind::Runtime);
    assert_eq!(regular_overflow.message, overflow.message);

    let fold_overflow = eval("(fold + 0 [9223372036854775807 1])").unwrap_err();
    assert_eq!(fold_overflow.kind, ErrorKind::Runtime);
    assert_eq!(fold_overflow.message, overflow.message);

    let div_overflow = eval("(/ -9223372036854775808 -1)").unwrap_err();
    assert_eq!(div_overflow.kind, ErrorKind::Runtime);
    assert_eq!(div_overflow.message, "i64 overflow");

    let non_exhaustive = eval("(match 3 (1 10) (2 20))").unwrap_err();
    assert_eq!(non_exhaustive.kind, ErrorKind::Runtime);
}

#[test]
fn tail_call_optimization_and_with_eval_order() {
    let mut alloc = HeapAllocator;
    let mut m = Machine::new(
        Limits {
            steps: 100_000,
            depth: 10,
            memory_bytes: 16 * 1024 * 1024,
        },
        &mut alloc,
    );
    // depth 上限 10 でも 500 回の末尾再帰呼び出しが TooDeep にならず完了する。
    let sum = m
        .eval(
            r#"
            (defn tail_sum (n acc)
              (if (== n 0) acc (tail_sum (- n 1) (+ acc n))))
            (tail_sum 500 0)
            "#,
        )
        .unwrap();
    assert_eq!(sum, Value::Int(125_250));

    // `with` は式の評価前に構造体型検査と各フィールド存在検査を行う。
    let err1 = eval("(with 1 { a: nosuch })").unwrap_err();
    assert!(err1.message.contains("requires a struct"), "{err1}");

    let err2 = eval("(with Card { cost: 1 } { missing: (/ 1 0) })").unwrap_err();
    assert!(err2.message.contains("no field 'missing'"), "{err2}");
}

#[test]
fn cross_function_tail_calls_and_first_class_and_or() {
    let src = r#"
        (defn g (x) (let (a 1 b 2) (+ x a b)))
        (defn f (x) (g x))
        (defn h () (g 10))
        (+ (f 1) (h))
    "#;
    assert_eq!(eval(src).unwrap(), Value::Int(17));

    let diff_arity = r#"
        (defn g3 (x y z) (+ x y z))
        (defn f0 () (g3 1 2 3))
        (f0)
    "#;
    assert_eq!(eval(diff_arity).unwrap(), Value::Int(6));

    let mut alloc = HeapAllocator;
    let mut m = Machine::new(Limits::default(), &mut alloc);
    m.load_module(
        "m",
        r#"
        (defn helper (x) (let (k 10) (+ x k)))
        (defn entry (x) (helper x))
        "#,
    )
    .unwrap();
    assert_eq!(
        m.call_in("m", "entry", &[Value::Int(5)]).unwrap(),
        Value::Int(15)
    );

    assert_eq!(
        eval("(let (f and) (f true false))").unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        eval("(let (f or) (f false true))").unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn empty_bodies_and_def_errors_and_frame_sizes() {
    for bad in [
        "(fn ())",
        "(defn f ())",
        "(let (x 1))",
        "(when true)",
        "(match 1 (x))",
        "[1 (when true) 3]",
        "(let (g (fn (a))) [7 (g 1) 8])",
    ] {
        let err = eval(bad).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse, "expected Parse for {bad}");
        assert_eq!(err.message, "body cannot be empty", "for {bad}");
    }

    for bad_def in ["(def)", "(def 1 2)"] {
        let err = eval(bad_def).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
        assert_eq!(err.message, "expected variable name");
    }

    assert!(
        crate::parser::parse_frame_size() <= 64,
        "ParseFrame size: {}",
        crate::parser::parse_frame_size()
    );
    assert!(
        crate::compiler::step_size() <= 48,
        "Step size: {}",
        crate::compiler::step_size()
    );
}
