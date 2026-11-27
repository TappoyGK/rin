//! 名前表の切替・シャドーイングの復元・捕捉番号の順序を検証する。
use rin::{Builder, HeapAllocator, Limits, Value};

#[test]
fn shadowed_names_are_restored_after_let_and_match() {
    for width in [0, 8, 9, 100] {
        for depth in [0, 12] {
            let bindings = (0..width).map(|i| format!("v{i} {i} ")).collect::<String>();
            let source = format!(
                "{}(let (x 7 {bindings}) [(let (x 9 x 11) x) x (match some(3) (some(x) x) (_ 0)) x]){}",
                "(let () ".repeat(depth),
                ")".repeat(depth),
            );
            let mut alloc = HeapAllocator;
            let mut m = Builder::new(Limits::default(), &mut alloc);
            assert_eq!(
                m.eval(&source).unwrap(),
                Value::list(vec![
                    Value::Int(11),
                    Value::Int(7),
                    Value::Int(3),
                    Value::Int(7)
                ])
            );
        }
    }
}

#[test]
fn nested_captures_keep_first_use_order_and_outer_bindings() {
    for width in [1, 8, 9, 100] {
        for depth in [0, 12] {
            let bindings = (0..width).map(|i| format!("v{i} {i} ")).collect::<String>();
            let values = (0..width)
                .rev()
                .map(|i| format!("v{i} "))
                .collect::<String>();
            let source = format!(
                "{}(let ({bindings}) (let (f ((fn () (fn () [{values} v0])))) (let (v0 -1) (f)))){}",
                "(let () ".repeat(depth),
                ")".repeat(depth),
            );
            let mut expected = (0..width).rev().map(Value::Int).collect::<Vec<_>>();
            expected.push(Value::Int(0));
            let mut alloc = HeapAllocator;
            let mut m = Builder::new(Limits::default(), &mut alloc);
            assert_eq!(m.eval(&source).unwrap(), Value::list(expected));
        }
    }
}

#[test]
fn many_globals_and_repeated_string_constants_keep_their_numbers() {
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(Limits::default(), &mut alloc);
    let definitions = (0..100)
        .map(|i| format!("(def global{i} {i}) "))
        .collect::<String>();
    let strings = (0..100)
        .rev()
        .map(|i| format!("\"text{i}\" \"text{i}\" "))
        .collect::<String>();
    let expected = Value::list(
        (0..100)
            .rev()
            .flat_map(|i| {
                [
                    Value::text(format!("text{i}")),
                    Value::text(format!("text{i}")),
                ]
            })
            .collect(),
    );
    assert_eq!(
        m.eval(&format!("{definitions}[{strings}]")).unwrap(),
        expected
    );
    assert_eq!(
        m.eval("(def global100 999) (+ global100 global99)")
            .unwrap(),
        Value::Int(1098)
    );
}

#[test]
fn cached_symbol_numbers_reject_redefinitions_and_allow_new_names() {
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(Limits::default(), &mut alloc);
    m.eval("(defn first () 1) (defn second () 2)").unwrap();
    for _ in 0..2 {
        assert_eq!(m.eval("(first)").unwrap(), Value::Int(1));
        assert_eq!(m.eval("(second)").unwrap(), Value::Int(2));
    }
    let err = m.eval("(defn second () 3)").unwrap_err();
    assert_eq!(err.rule(), "expr.defn");
    assert_eq!(m.eval("(second)").unwrap(), Value::Int(2));
    assert!(m.eval("(later)").is_err());
    m.eval("(defn later () 4)").unwrap();
    assert_eq!(m.eval("(later)").unwrap(), Value::Int(4));
}
