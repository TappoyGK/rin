use rin::{ErrorKind, HeapAllocator, Limits, Machine, Value};

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
        "(== LongName.Variant(xs) LongName.Variant(xs))",
        "(append xs (reverse xs))",
        "(zip xs xs)",
        "[xs xs xs]",
        "xs[2]",
    ];
    for source in sources {
        let xs = Value::list(vec![Value::Int(3), Value::Int(1), Value::Int(2)]);
        let expected = rin::eval_with(source, &[("xs", xs.clone())], Limits::default()).unwrap();
        for steps in 0..=100 {
            let mut alloc = HeapAllocator;
            let mut m = Machine::new(
                Limits {
                    steps,
                    ..Limits::default()
                },
                &mut alloc,
            );
            m.bind("xs", &xs).unwrap();
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
