use rin::test::{TestStatus, run, run_filtered};
use rin::{Builder, ErrorKind, HeapAllocator, Limits, Value, eval};

#[test]
fn collects_checks_in_control_flow_and_continues_after_failure() {
    let source = r#"
(def x 5)
(deftest "multiple"
  (let (n 2)
    (check_eq (+ x n) 8)
    (check false)
    (if false (check false) (check true))
    (check_eq (check_eq n 2) (when false 1))))
(deftest "next" (check_eq x 5))
"#;
    let r = run(&[("checks.rin", source)], &[], Limits::default());
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.tests.len(), 2);
    assert_eq!(r.tests[0].status, TestStatus::Failed);
    assert_eq!(r.tests[0].failures.len(), 2);
    let f = &r.tests[0].failures[0];
    assert_eq!(f.actual, Value::Int(7));
    assert_eq!(f.expected, Value::Int(8));
    assert_eq!((f.at.line, f.at.col), (5, 5));
    assert_eq!(r.tests[1].status, TestStatus::Passed);
}

#[test]
fn tests_get_fresh_environments_and_step_budgets() {
    let source = r#"
(def x 1)
(deftest "first" (def x 2) (check_eq x 2))
(deftest "second" (check_eq x 1))
"#;
    assert!(run(&[("fresh", source)], &[], Limits::default()).is_success());
    let source = "(deftest \"a\" (check true)) (deftest \"b\" (check true))";
    let report = run(
        &[("steps", source)],
        &[],
        Limits {
            steps: 2,
            ..Limits::default()
        },
    );
    assert!(report.is_success(), "{report:?}");
    let report = run(
        &[("steps", source)],
        &[],
        Limits {
            steps: 1,
            ..Limits::default()
        },
    );
    assert!(
        report
            .tests
            .iter()
            .all(|t| t.error.as_ref().unwrap().kind == ErrorKind::TooManySteps)
    );
}

#[test]
fn runtime_error_preserves_prior_failures_and_other_tests_run() {
    let r = run(
        &[(
            "errors",
            "(deftest \"bad\" (check false) (/ 1 0)) (deftest \"ok\" (check true))",
        )],
        &[],
        Limits::default(),
    );
    assert_eq!(r.tests[0].status, TestStatus::Error);
    assert_eq!(r.tests[0].failures.len(), 1);
    assert_eq!(r.tests[0].error.as_ref().unwrap().kind, ErrorKind::Runtime);
    assert_eq!(r.tests[1].status, TestStatus::Passed);
    let r = run(
        &[("types", "(deftest \"bad\" (check_eq 1 true))")],
        &[],
        Limits::default(),
    );
    assert_eq!(r.tests[0].error.as_ref().unwrap().rule(), "op.cmp");
}

#[test]
fn test_syntax_is_rejected_outside_its_context() {
    for source in [
        "(deftest \"x\" (check true))",
        "(if false (check true) 1)",
        "(check_eq 1 1)",
    ] {
        assert_eq!(eval(source).unwrap_err().kind, ErrorKind::Parse);
        let mut alloc = HeapAllocator;
        let mut machine = Builder::new(Limits::default(), &mut alloc);
        assert_eq!(
            machine.load_module("m", source).unwrap_err().kind,
            ErrorKind::Parse
        );
    }
    for source in [
        "(check true)",
        "(deftest 1 (check true))",
        "(deftest \"x\" (check))",
        "(deftest \"x\" (check_eq 1))",
        "(deftest \"x\" (check true false))",
        "(deftest \"x\" (if false (deftest \"nested\") 1))",
        "(deftest \"x\") (deftest \"x\")",
        "(defn helper () (check true))",
        "(deftest \"x\" (let (f check) (f true)))",
    ] {
        let r = run(&[("bad", source)], &[], Limits::default());
        assert!(!r.errors.is_empty(), "{source}: {r:?}");
        assert!(r.tests.is_empty());
    }
}

#[test]
fn modules_are_loaded_by_dependency_and_errors_have_sources() {
    let modules = [
        ("a", "(import b) (defn double (x) (+ x b.n))"),
        ("b", "(def n 3)"),
    ];
    let r = run(
        &[(
            "modules",
            "(import a) (deftest \"works\" (check_eq (a.double 3) 6))",
        )],
        &modules,
        Limits::default(),
    );
    assert!(r.is_success(), "{r:?}");
    for modules in [
        vec![("a", "(import missing)")],
        vec![("a", "(import b)"), ("b", "(import a)")],
        vec![("a", "1"), ("a", "2")],
    ] {
        let r = run(&[], &modules, Limits::default());
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].error.rule(), "test.modules");
    }
    let r = run(
        &[("t", "(import m) (deftest \"error\" (m.fail))")],
        &[("m", "(defn fail () (/ 1 0))")],
        Limits::default(),
    );
    assert_eq!(r.tests[0].error.as_ref().unwrap().module(), Some("m"));
    assert!(!r.tests[0].error.as_ref().unwrap().backtrace().is_empty());
}

#[test]
fn filters_skip_execution_but_not_syntax_validation() {
    let r = run_filtered(
        &[(
            "filter",
            "(deftest \"bad\" (/ 1 0)) (deftest \"good\" (check true))",
        )],
        &[],
        Limits::default(),
        Some("oo"),
    );
    assert_eq!(r.tests.len(), 1);
    assert_eq!(r.tests[0].name, "good");
    assert!(r.is_success());
    let r = run_filtered(
        &[("bad", "(deftest \"bad\" (check))")],
        &[],
        Limits::default(),
        Some("good"),
    );
    assert!(!r.is_success());
}

#[test]
fn checks_work_in_closures_and_iterative_compilation() {
    let nested = format!(
        "{}(check false){}",
        "(when true ".repeat(50),
        ")".repeat(50)
    );
    let source = format!("(deftest \"deep\" {nested} (map (fn (x) (check_eq x 1)) [1, 2, 3]))");
    let r = run(&[("deep", &source)], &[], Limits::default());
    assert_eq!(r.tests[0].status, TestStatus::Failed);
    assert_eq!(r.tests[0].failures.len(), 3);
}

#[test]
fn depth_and_memory_limits_are_errors() {
    let r = run(
        &[(
            "depth",
            "(defn f (x) (+ 1 (f x))) (deftest \"depth\" (f 0))",
        )],
        &[],
        Limits {
            depth: 4,
            ..Limits::default()
        },
    );
    assert_eq!(r.tests[0].error.as_ref().unwrap().kind, ErrorKind::TooDeep);
    let r = run(
        &[(
            "memory",
            "(deftest \"memory\" (check_eq [1, 2, 3] [1, 2, 3]))",
        )],
        &[],
        Limits {
            memory_bytes: 0,
            ..Limits::default()
        },
    );
    assert_eq!(
        r.tests[0].error.as_ref().unwrap().kind,
        ErrorKind::TooMuchMemory
    );
}

#[test]
fn test_form_names_can_be_bound_as_ordinary_values() {
    for source in [
        r#"(deftest "x" (let (check 1) (check_eq check 1)))"#,
        r#"(deftest "y" (let (f (fn (check) check)) (check_eq (f 2) 2)))"#,
        r#"(deftest "capture" (let (check 3 f (fn () check)) (check_eq (f) 3)))"#,
        r#"(deftest "match" (match some(4) (some(check) (check_eq check 4))))"#,
        r#"(def check 5) (deftest "global" (check_eq check 5))"#,
        r#"(deftest "names" (let (deftest 1 check_eq 2) (check (== (+ deftest check_eq) 3))))"#,
    ] {
        let report = run(&[("bindings", source)], &[], Limits::default());
        assert!(report.is_success(), "{source}: {report:?}");
    }
    for source in [
        r#"(deftest "scope" (let (check 1) check) (let (c check) c))"#,
        r#"(deftest "init" (let (check check) check))"#,
        r#"(deftest "param" (fn (check) check) (let (c check) c))"#,
        r#"(deftest "match" (match some(4) (some(check) check)) (let (c check) c))"#,
    ] {
        let report = run(&[("scope", source)], &[], Limits::default());
        assert_eq!(
            report.errors[0].error.message, "test forms cannot be used as values",
            "{source}"
        );
    }
}

#[test]
fn check_placement_and_arity_have_distinct_diagnostics() {
    for source in ["(check true)", "(defn f () (check true))", "(check_eq 1 1)"] {
        let report = run(&[("placement", source)], &[], Limits::default());
        assert_eq!(
            report.errors[0].error.message,
            "checks are only allowed inside deftest"
        );
    }
    for (body, message) in [
        ("(check)", "check requires 1 argument(s), got 0"),
        ("(check_eq 1)", "check_eq requires 2 argument(s), got 1"),
        ("(check true false)", "check requires 1 argument(s), got 2"),
    ] {
        let source = format!("(deftest \"arity\" {body})");
        let report = run(&[("arity", &source)], &[], Limits::default());
        assert_eq!(report.errors[0].error.message, message);
    }
}
