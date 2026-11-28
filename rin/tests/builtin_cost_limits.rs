use rin::{Builder, ErrorKind, HeapAllocator, Limits, Value};

#[test]
fn every_limit_releases_temporaries_and_preserves_bound_values() {
    let sources = [
        "(map (fn (x) [x x]) xs)",
        "(filter (fn (x) (== x 1)) xs)",
        "(flat_map (fn (x) (range 0 x)) xs)",
        "(sort_by (fn (x) (if (== x 1) \"bb\" \"aa\")) xs)",
        "(fold (fn (a x) (push a x)) [] xs)",
        "(== [xs xs] [xs xs])",
        "(== LongName { long_field: xs } LongName { long_field: xs })",
        "(== LongNameV.Variant(xs) LongNameV.Variant(xs))",
        "(append xs (reverse xs))",
        "(zip xs xs)",
        "[xs xs xs]",
        "xs[2]",
    ];
    let schema_prelude = r#"
        (defschema LongName (struct LongName (long_field (list i64))))
        (defschema LongNameV (variant LongNameV (Variant (list i64))))
    "#;
    for source in sources {
        let xs = Value::list(vec![Value::Int(3), Value::Int(1), Value::Int(2)]);
        let expected = {
            let mut alloc = HeapAllocator;
            let mut m = Builder::new(Limits::default(), &mut alloc);
            m.eval(schema_prelude).unwrap();
            m.bind("xs", "(list i64)", &xs).unwrap();
            m.eval(source).unwrap()
        };
        for steps in 0..=100 {
            let mut alloc = HeapAllocator;
            let mut m = Builder::new(
                Limits {
                    steps,
                    ..Limits::default()
                },
                &mut alloc,
            );
            m.eval(schema_prelude).unwrap();
            m.bind("xs", "(list i64)", &xs).unwrap();
            let baseline = m.live_bytes();
            for _ in 0..3 {
                match m.eval(source) {
                    Ok(value) => assert_eq!(value, expected),
                    Err(e) => assert_eq!(e.kind, ErrorKind::TooManySteps, "{source}: {e}"),
                }
                assert_eq!(m.live_bytes(), baseline, "{source}, steps={steps}");
            }
        }
    }
}

#[test]
fn fast_integer_calls_and_callback_calls_charge_the_same_internal_work() {
    for (op, cost) in [
        ("+", 1),
        ("-", 1),
        ("*", 1),
        ("<", 1),
        ("<=", 1),
        (">", 1),
        (">=", 1),
        ("==", 2),
        ("!=", 2),
    ] {
        let source = format!("({op} 1 2)");
        for steps in 0..=4 + cost {
            let result = rin::eval_with(
                &source,
                &[],
                Limits {
                    steps,
                    ..Limits::default()
                },
            );
            assert_eq!(result.is_ok(), steps == 4 + cost, "{op}, {steps}");
        }
    }
}

#[test]
fn with_charges_original_fields_before_copying() {
    use std::collections::BTreeMap;
    for n in [0usize, 1, 100] {
        let fields: BTreeMap<_, _> = (0..n)
            .map(|i| (format!("f{i}"), Value::some(Value::Int(i as i64))))
            .collect();
        let original = Value::structure("S", fields.clone());
        for update in [false, true] {
            if update && n == 0 {
                continue;
            }
            let (source, cost) = if update {
                ("(with s { f0: some(7) })", n + 4)
            } else {
                ("(with s {})", n + 2)
            };
            let mut expected = fields.clone();
            if update {
                expected.insert("f0".into(), Value::some(Value::Int(7)));
            }
            let expected = Value::structure("S", expected);
            let field_defs: String = (0..n).map(|i| format!(" (f{i} (option i64))")).collect();
            let schema_def = format!("(defschema S (struct S{field_defs}))");
            for steps in 0..=cost {
                let mut alloc = HeapAllocator;
                let mut m = Builder::new(
                    Limits {
                        steps,
                        ..Limits::default()
                    },
                    &mut alloc,
                );
                m.eval(&schema_def).unwrap();
                m.bind("s", "S", &original).unwrap();
                let baseline = m.live_bytes();
                for _ in 0..3 {
                    let result = m.eval(source);
                    if steps < cost {
                        let error = result.unwrap_err();
                        assert_eq!(error.kind, ErrorKind::TooManySteps);
                        if steps >= cost - n {
                            assert_eq!((error.at.line, error.at.col), (1, 1));
                        }
                    } else {
                        assert_eq!(result.unwrap(), expected);
                    }
                    assert_eq!(m.live_bytes(), baseline);
                }
            }
        }
    }
}
