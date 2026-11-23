//! ホスト呼び出しで式評価分を除き、組み込み関数の計上式を検証する。
use rin::{ErrorKind, HeapAllocator, Limits, Machine, Value};

fn check(name: &str, args: &[Value], cost: usize, expected: Value) {
    for steps in 0..=cost + 1 {
        let mut alloc = HeapAllocator;
        let mut machine = Machine::new(
            Limits {
                steps,
                ..Limits::default()
            },
            &mut alloc,
        );
        for _ in 0..3 {
            let result = machine.call(name, args);
            if steps < cost {
                let e = result.unwrap_err();
                assert_eq!(e.kind, ErrorKind::TooManySteps, "{name}, {steps}");
                assert_eq!((e.at.line, e.at.col), (0, 0));
            } else {
                assert_eq!(result.unwrap(), expected, "{name}, {steps}");
            }
            assert_eq!(machine.live_bytes(), 0, "{name}, {steps}");
        }
    }
}

fn ints(n: usize) -> Vec<Value> {
    (0..n).map(|i| Value::Int(i as i64)).collect()
}
fn builtin(name: &'static str) -> Value {
    Value::Builtin(name)
}

#[test]
fn collection_costs_at_zero_one_and_n() {
    for n in [0, 1, 17] {
        let values = ints(n);
        let xs = Value::list(values.clone());
        let arr = Value::array(values.clone());
        let length = Value::Int(n as i64);
        let one = std::slice::from_ref(&xs);
        check("len", one, 1, length.clone());
        check("is_empty", one, 1, Value::Bool(n == 0));
        check("len", std::slice::from_ref(&arr), 1, length.clone());
        check("list", &values, 1 + n, xs.clone());
        check("range", &[Value::Int(0), length.clone()], 1 + n, xs.clone());
        check(
            "reverse",
            one,
            1 + n,
            Value::list(values.iter().rev().cloned().collect()),
        );
        check(
            "car",
            one,
            1,
            values
                .first()
                .cloned()
                .map(Value::some)
                .unwrap_or(Value::None),
        );
        check(
            "last",
            one,
            1 + n,
            values
                .last()
                .cloned()
                .map(Value::some)
                .unwrap_or(Value::None),
        );
        check(
            "cdr",
            one,
            1,
            Value::list(values.iter().skip(1).cloned().collect()),
        );
        let mut pushed = values.clone();
        pushed.push(Value::Int(99));
        check(
            "push",
            &[xs.clone(), Value::Int(99)],
            2 + 2 * n,
            Value::list(pushed),
        );
        let mut cons = vec![Value::Int(99)];
        cons.extend(values.clone());
        check("cons", &[Value::Int(99), xs.clone()], 1, Value::list(cons));
        check(
            "append",
            &[xs.clone(), xs.clone()],
            3 + 4 * n,
            Value::list(values.iter().chain(&values).cloned().collect()),
        );
        check(
            "flatten",
            &[Value::list(vec![xs.clone(), xs.clone()])],
            3 + 4 * n,
            Value::list(values.iter().chain(&values).cloned().collect()),
        );
        check("take", &[xs.clone(), length.clone()], 1 + 2 * n, xs.clone());
        check(
            "slice",
            &[xs.clone(), Value::Int(0), length.clone()],
            1 + 2 * n,
            xs.clone(),
        );
        check(
            "drop",
            &[xs.clone(), length.clone()],
            1 + n,
            Value::list(vec![]),
        );
        check("get", &[xs.clone(), length.clone()], 1 + n, Value::None);
        check("get", &[arr.clone(), length], 1, Value::None);
        check("list->array", one, 1 + n, arr.clone());
        check("array->list", &[arr], 1 + 2 * n, xs.clone());
        let pairs = Value::list(
            values
                .iter()
                .map(|v| Value::tuple(vec![v.clone(), v.clone()]))
                .collect(),
        );
        check("zip", &[xs.clone(), xs.clone()], 1 + 3 * n, pairs.clone());
        check("enumerate", one, 1 + 2 * n, pairs);
        check(
            "contains",
            &[xs.clone(), Value::Int(-1)],
            1 + 2 * n,
            Value::Bool(false),
        );
        check(
            "unique",
            one,
            1 + 2 * n + n * n.saturating_sub(1) / 2,
            xs.clone(),
        );
        let log = if n < 2 {
            0
        } else {
            (usize::BITS - (n - 1).leading_zeros()) as usize
        };
        check("sort", one, 1 + 2 * n + n * log, xs.clone());
        check(
            "sort",
            &[Value::list(values.iter().rev().cloned().collect())],
            1 + 2 * n + n * log,
            xs.clone(),
        );
        check(
            "sort_by",
            &[builtin("abs"), xs.clone()],
            1 + 3 * n + n * log,
            xs.clone(),
        );
        check("map", &[builtin("abs"), xs.clone()], 1 + 3 * n, xs.clone());
        check(
            "fold",
            &[builtin("+"), Value::Int(0), xs],
            1 + 2 * n,
            Value::Int((n * n.saturating_sub(1) / 2) as i64),
        );
    }
}

#[test]
fn callbacks_short_circuit_and_release_generated_values() {
    for n in [0, 1, 17] {
        let falses = Value::list(vec![Value::Bool(false); n]);
        check(
            "any",
            &[builtin("not"), falses.clone()],
            1 + n + usize::from(n > 0),
            Value::Bool(n > 0),
        );
        check(
            "all",
            &[builtin("not"), falses.clone()],
            1 + 2 * n,
            Value::Bool(true),
        );
        check(
            "find",
            &[builtin("not"), falses.clone()],
            1 + n + usize::from(n > 0),
            if n > 0 {
                Value::some(Value::Bool(false))
            } else {
                Value::None
            },
        );
        check(
            "filter",
            &[builtin("not"), falses.clone()],
            1 + 3 * n,
            falses,
        );
        let lists = Value::list(vec![Value::list(vec![Value::Int(1), Value::Int(2)]); n]);
        check(
            "flat_map",
            &[builtin("reverse"), lists],
            1 + 8 * n,
            Value::list(
                (0..n)
                    .flat_map(|_| [Value::Int(2), Value::Int(1)])
                    .collect(),
            ),
        );
    }
}

#[test]
fn strings_nested_equality_and_scalar_operations() {
    for n in [0, 1, 17] {
        check(
            "len",
            &[Value::str("あ".repeat(n))],
            1 + 3 * n,
            Value::Int(n as i64),
        );
        let a = Value::str(format!("{}a", "x".repeat(n)));
        let b = Value::str(format!("{}b", "x".repeat(n)));
        check("==", &[a.clone(), b.clone()], 3 + n, Value::Bool(false));
        check("!=", &[a.clone(), b.clone()], 3 + n, Value::Bool(true));
        check(
            "sort",
            &[Value::list(vec![b.clone(), a.clone()])],
            5 + 2 * (n + 2),
            Value::list(vec![a, b]),
        );
        check(
            "==",
            &[Value::array(ints(n)), Value::array(ints(n))],
            2 + 2 * n,
            Value::Bool(true),
        );
        let mut a = Value::Int(1);
        let mut b = Value::Int(1);
        for _ in 0..n {
            a = Value::some(a);
            b = Value::some(b);
        }
        check("==", &[a, b], 2 + n, Value::Bool(true));
    }
    check("+", &ints(5), 4, Value::Int(10));
    check(
        "and",
        &[Value::Bool(true), Value::Bool(false), Value::Bool(true)],
        3,
        Value::Bool(false),
    );
    check(
        "or",
        &[Value::Bool(false), Value::Bool(true), Value::Bool(false)],
        3,
        Value::Bool(true),
    );
    check("==", &[Value::Int(1), Value::Int(1)], 2, Value::Bool(true));
    check("sqrt", &[Value::Float(4.0)], 1, Value::Float(2.0));
}

#[test]
fn huge_range_rejects_overflow_before_allocating() {
    let mut alloc = HeapAllocator;
    let mut m = Machine::new(
        Limits {
            steps: usize::MAX,
            ..Limits::default()
        },
        &mut alloc,
    );
    assert_eq!(
        m.call("range", &[Value::Int(i64::MIN), Value::Int(i64::MAX)])
            .unwrap_err()
            .kind,
        ErrorKind::TooManySteps
    );
    assert_eq!(m.live_bytes(), 0);
}

#[test]
fn unique_batches_only_scalar_comparisons_within_the_remaining_budget() {
    for n in [0, 1, 17] {
        let output = if n == 0 { vec![] } else { vec![Value::Int(7)] };
        check(
            "unique",
            &[Value::list(vec![Value::Int(7); n])],
            1 + 2 * n,
            Value::list(output),
        );
    }
    // 型の違うスカラーとオブジェクトを混ぜても、比較順・費用を保つ。
    // 走査5、比較1+1+2+3、生成3、呼び出し1 = 16。
    let xs = vec![
        Value::Int(1),
        Value::some(Value::Int(1)),
        Value::Int(1),
        Value::Bool(false),
        Value::some(Value::Int(1)),
    ];
    check(
        "unique",
        &[Value::list(xs)],
        16,
        Value::list(vec![
            Value::Int(1),
            Value::some(Value::Int(1)),
            Value::Bool(false),
        ]),
    );
}

#[test]
fn struct_comparison_keeps_key_then_value_budget_order() {
    use std::collections::BTreeMap;
    for n in [0, 1, 17] {
        let fields: BTreeMap<_, _> = (0..n)
            .map(|i| (format!("k{i:02}"), Value::Int(i)))
            .collect();
        let a = Value::structure("S", fields.clone());
        let cost = 3 + 5 * n as usize;
        check("==", &[a.clone(), a.clone()], cost, Value::Bool(true));
        // 外側の比較スタックに残るリストの末尾も比較する。
        check(
            "==",
            &[
                Value::list(vec![a.clone(), Value::Int(1)]),
                Value::list(vec![a.clone(), Value::Int(1)]),
            ],
            cost + 4,
            Value::Bool(true),
        );
        for i in 0..n {
            let mut other = fields.clone();
            other.insert(format!("k{i:02}"), Value::Int(-1));
            check(
                "==",
                &[a.clone(), Value::structure("S", other)],
                3 + 4 * n as usize + i as usize + 1,
                Value::Bool(false),
            );
        }
        if n > 0 {
            let mut other = fields.clone();
            other.remove("k00");
            other.insert("j00".into(), Value::Int(-1));
            check(
                "==",
                &[a.clone(), Value::structure("S", other)],
                3 + 4 * n as usize,
                Value::Bool(false),
            );
            // オブジェクトに遭遇したら通常の比較へ戻り、二重計上しない。
            let mut nested = fields.clone();
            nested.insert("k00".into(), Value::some(Value::Int(0)));
            let value = Value::structure("S", nested);
            check("==", &[value.clone(), value], cost + 1, Value::Bool(true));
        }
    }
}
