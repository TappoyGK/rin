use rin::{eval, fmt::format};

fn formatted(source: &str) -> String {
    let out = format(source).unwrap_or_else(|e| panic!("{source}\n{e}"));
    assert_eq!(format(&out).unwrap(), out, "not idempotent: {source}");
    out
}

#[test]
fn basic_whitespace_literals_comments_and_definitions() {
    assert_eq!(formatted(" ( +  1  2 )\n\n"), "(+ 1 2)\n");
    assert_eq!(formatted("(def a 1)(def b 2)"), "(def a 1)\n\n(def b 2)\n");
    assert_eq!(
        formatted(";;hello\n(def a 1);tail\n;;;doc\n(def b 2)"),
        ";; hello\n(def a 1) ; tail\n\n;;; doc\n(def b 2)\n"
    );
    assert_eq!(formatted("[ 1  2,3 ]"), "[1, 2, 3]\n");
    assert_eq!(formatted("(1,2) (1,)"), "(1, 2)\n(1,)\n");
    assert_eq!(
        formatted("Foo{ x :1 y: [2 3]}"),
        "Foo { x: 1, y: [2, 3] }\n"
    );
    for source in [
        "\"; \\\"\\n猫\"",
        "'\\n'",
        "\"line1\n  line2\"",
        "nan",
        "0xF_F",
        "1_000e-2",
    ] {
        assert_eq!(formatted(source), format!("{source}\n"));
    }
}

#[test]
fn adjacency_and_postfix_are_preserved() {
    for source in [
        "some(1) ok(2) err(3) Kind.A(4)",
        "some (1)",
        "(let (xs [1 2]) xs[0])",
        "[1]; note\n[0]",
        "[(if true [1] [2])[0] Foo { x: [1] }.x[0] Kind.A(2)]",
        "[Foo ; comment\n{a: 1} x ; comment\n.y[0]]",
        "[some ; comment\n(1)]",
        "(with Foo {x: 1} {x: 2})",
        "(match some(1) (some(x) x) (_ 0))",
    ] {
        formatted(source);
    }
    let source = "(let (xs [1 2]) (+ xs[0] xs[1]))";
    assert_eq!(eval(source).unwrap(), eval(&formatted(source)).unwrap());
}

#[test]
fn long_forms_wrap_and_keep_pairs_and_comments() {
    let long = "x".repeat(65);
    for source in [
        format!("(defn foo (a b) (+ a b) (list \"{long}\" \"{long}\"))"),
        format!("(let (a \"{long}\" b \"{long}\") [a b])"),
        format!("(if true \"{long}\" \"{long}\")"),
        format!("(foo \"{long}\" \"{long}\")"),
        format!("[\"{long}\" \"{long}\"]"),
        format!("Foo {{ a: \"{long}\" b: \"{long}\" }}"),
        "(defn foo (x)\n;;hello\n(+ x 1);end\n)".into(),
        "(deftest \"name\" ; comment\n(check true)\n(check_eq 1 1))".into(),
    ] {
        let out = formatted(&source);
        assert!(out.lines().all(|line| line.chars().count() <= 100), "{out}");
    }
}

#[test]
fn invalid_sources_are_rejected() {
    for source in [
        "(",
        "[",
        "\"unterminated",
        "(if true 1)",
        "(deftest 1)",
        "(check true)",
    ] {
        assert!(format(source).is_err(), "{source}");
    }
}

#[test]
fn every_valid_spec_case_is_idempotent() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/spec-cases");
    let mut count = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let content = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for block in content.split("```rin\n").skip(1) {
            let source = block.split("```").next().unwrap();
            match format(source) {
                Ok(out) => {
                    assert_eq!(format(&out).unwrap(), out, "{source}");
                    count += 1;
                }
                Err(error) => assert_ne!(error.rule(), "fmt.preserve", "{source}: {error}"),
            }
        }
    }
    assert!(count >= 25, "only {count} valid examples");
}

#[test]
fn nested_forms_width_boundaries_and_comments_are_stable() {
    for length in [1, 50, 85, 90, 94, 95, 96, 97, 98] {
        let text = "x".repeat(length);
        for source in [
            format!("(foo [\"{text}\"] 1)"),
            format!("(foo (+ \"{text}\" 1) 2)"),
            format!(
                "(defn foo ({} ) 1)",
                (0..50)
                    .map(|i| format!("arg{i}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            format!("(match 1 (1 \"{text}\") (_ \"{text}\"))"),
        ] {
            formatted(&source);
        }
    }
    let seeds = [
        "1",
        "a.b[0]",
        "some([1 2])",
        "(1,)",
        "Foo {a: 1 b: [2 3]}",
        "(if true 1 2)",
        "(fn (x) x)",
        "Kind.A(1)",
    ];
    for a in seeds {
        for b in seeds {
            for source in [
                format!("[{a};between\n{b}]"),
                format!("(f {a} ;;between\n{b})"),
                format!("(let (a {a}\n\n b {b}) [a b])"),
                format!("Foo {{a:{a};field\nb:{b}}}"),
            ] {
                formatted(&source);
            }
        }
    }
    let deep = format!("{}1{}", "(when true ".repeat(150), ")".repeat(150));
    formatted(&deep);
    let too_deep = format!("{}1{}", "(when true ".repeat(1000), ")".repeat(1000));
    assert!(format(&too_deep).is_err());
}
