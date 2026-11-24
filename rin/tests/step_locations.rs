//! ステップ上限を各実行位置で超えさせ、コンパイル後も正確な位置が残ることを確認する。
use rin::{ErrorKind, HeapAllocator, Limits, Machine, Value};

fn check_every_limit(source: &str, positions: &[(usize, usize)], expected: Value) {
    for limit in 0..=positions.len() {
        let mut alloc = HeapAllocator;
        let mut machine = Machine::new(
            Limits {
                steps: limit,
                ..Limits::default()
            },
            &mut alloc,
        );
        let result = machine.eval(source);
        if let Some(&(line, col)) = positions.get(limit) {
            let error = result.unwrap_err();
            assert_eq!(
                error.kind,
                ErrorKind::TooManySteps,
                "limit={limit}, {source}"
            );
            assert_eq!(error.rule(), "eval.limits");
            assert_eq!(
                (error.at.line, error.at.col),
                (line, col),
                "limit={limit}, {source}"
            );
        } else {
            assert_eq!(result.unwrap(), expected, "limit={limit}, {source}");
        }
    }
}

#[test]
fn exact_step_locations_across_branches_and_function_scopes() {
    check_every_limit(
        "(let (x 3)\n  (if true\n      (+ x 2)\n      (/ 1 0)))",
        &[
            (1, 1),
            (1, 9),
            (2, 3),
            (2, 7),
            (3, 7),
            (3, 8),
            (3, 10),
            (3, 12),
            (3, 7),
        ],
        Value::Int(5),
    );
    check_every_limit(
        "(if false\n    (/ 1 0)\n    (+ 1 2))",
        &[(1, 1), (1, 5), (3, 5), (3, 6), (3, 8), (3, 10), (3, 5)],
        Value::Int(3),
    );
    check_every_limit(
        "(defn sq (x)\n  (* x x))\n(sq 7)",
        &[
            (1, 1),
            (3, 1),
            (3, 2),
            (3, 5),
            (2, 3),
            (2, 4),
            (2, 6),
            (2, 8),
            (2, 3),
        ],
        Value::Int(49),
    );
    check_every_limit(
        "(let (x 1)\n  (and true (> x 0))\n  (or false (< x 0))\n  (match x (1 (+ x 2)) (_ 0)))",
        &[
            (1, 1),
            (1, 9),
            (2, 3),
            (2, 8),
            (2, 13),
            (2, 14),
            (2, 16),
            (2, 18),
            (2, 13),
            (3, 3),
            (3, 7),
            (3, 13),
            (3, 14),
            (3, 16),
            (3, 18),
            (3, 13),
            (4, 3),
            (4, 10),
            (4, 15),
            (4, 16),
            (4, 18),
            (4, 20),
            (4, 15),
        ],
        Value::Int(3),
    );
}

#[test]
fn exact_step_locations_when_compiler_switches_to_iterative_traversal() {
    let source = format!("{}(+ 1 2){}", "(let (x 1)\n".repeat(12), ")".repeat(12));
    let mut positions = Vec::new();
    for line in 1..=12 {
        positions.extend([(line, 1), (line, 9)]);
    }
    positions.extend([(13, 1), (13, 2), (13, 4), (13, 6), (13, 1)]);
    check_every_limit(&source, &positions, Value::Int(3));
}
