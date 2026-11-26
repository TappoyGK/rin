//! 診断規則と仕様の対応、各処理段階の代表的な失敗を検証する。
use rin::{ErrorKind, Schema, eval};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[test]
fn all_implementation_rules_are_documented() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut rules = BTreeSet::new();
    for doc in [
        "docs/spec.md",
        "docs/schema.md",
        "docs/testing.md",
        "docs/fmt.md",
    ] {
        for line in fs::read_to_string(root.join(doc)).unwrap().lines() {
            if let Some(rest) = line.trim_start().strip_prefix("**[") {
                let (id, _) = rest.split_once("]**").expect("closed rule definition");
                assert!(rules.insert(id.to_owned()), "duplicate rule {id}");
            }
        }
    }
    let mut pending = vec![root.join("rin/src")];
    let mut sites = 0;
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(fs::read_dir(path).unwrap().map(|e| e.unwrap().path()));
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        for constructor in [
            "Error::new(",
            "Error::lex(",
            "Error::parse(",
            "Error::runtime(",
            ".err_str(",
            ".err_fmt(",
            ".err_runtime(",
            ".err_runtime_fmt(",
            ".begin_msg(",
            ".err_lex(",
            ".err_lex_fmt(",
            ".err_parse(",
            ".err_parse_fmt(",
        ] {
            for rest in source.split(constructor).skip(1) {
                let rest = rest.trim_start();
                if path.ends_with("error/store.rs") && rest.starts_with("self.rule") {
                    continue;
                }
                if !rest.starts_with('"')
                    && matches!(
                        constructor,
                        ".err_str("
                            | ".err_fmt("
                            | ".err_runtime("
                            | ".err_runtime_fmt("
                            | ".begin_msg("
                    )
                {
                    // 転送ラッパー (例: `self.pool.core.err_str(rule, ...)`) はスキップする。
                    if rest.starts_with("rule") {
                        continue;
                    }
                }
                let literal = rest.strip_prefix('"').unwrap_or_else(|| {
                    panic!(
                        "{}: {constructor} must specify a literal rule",
                        path.display()
                    )
                });
                let (rule, _) = literal.split_once('"').unwrap();
                assert!(
                    rules.contains(rule),
                    "{}: undocumented rule {rule}",
                    path.display()
                );
                sites += 1;
            }
        }
        // C API の Host / phase エラーは Rust Error を介さず保存する。
        if path.ends_with("ffi/error.rs") || path.ends_with("ffi/mod.rs") {
            for pat in [
                "set_simple(",
                "record_simple_error(",
                "record_simple_error_fmt(",
            ] {
                for rest in source.split(pat).skip(1) {
                    if let Some(literal) = rest.trim_start().strip_prefix('"') {
                        let (rule, _) = literal.split_once('"').unwrap();
                        assert!(rules.contains(rule), "undocumented FFI rule {rule}");
                        sites += 1;
                    }
                }
            }
        }
    }
    assert!(sites > 100, "error construction sites were not scanned");
}

#[test]
fn language_diagnostics_and_display() {
    for (source, kind, rule) in [
        ("@", ErrorKind::Lex, "src.token"),
        ("\"\\q\"", ErrorKind::Lex, "src.escape"),
        ("'\\q'", ErrorKind::Lex, "src.escape"),
        ("9223372036854775808", ErrorKind::Lex, "src.int"),
        ("1e", ErrorKind::Lex, "src.float"),
        ("'a'b", ErrorKind::Lex, "src.char"),
        ("(", ErrorKind::Parse, "src.syntax"),
        ("(fn ())", ErrorKind::Parse, "expr.body"),
        ("(match 1 (1.5 0))", ErrorKind::Parse, "pat.kinds"),
        ("missing", ErrorKind::Runtime, "expr.name"),
        ("(if 1 2 3)", ErrorKind::Runtime, "expr.condition"),
        ("(when 1 2)", ErrorKind::Runtime, "expr.condition"),
        ("(import missing)", ErrorKind::Runtime, "expr.module"),
        ("(1)", ErrorKind::Runtime, "expr.call"),
        ("((fn (x) x))", ErrorKind::Runtime, "expr.call"),
        ("(+ 9223372036854775807 1)", ErrorKind::Runtime, "op.arith"),
        (
            "(def add +) (add 9223372036854775807 1)",
            ErrorKind::Runtime,
            "op.arith",
        ),
        ("(== 1 true)", ErrorKind::Runtime, "op.cmp"),
        ("(and 1 true)", ErrorKind::Runtime, "op.logic"),
        ("(as_i64 true)", ErrorKind::Runtime, "fn.convert"),
        ("(abs -9223372036854775808)", ErrorKind::Runtime, "fn.math"),
        ("(sort [true false])", ErrorKind::Runtime, "fn.list"),
        ("(filter (fn (x) x) [1])", ErrorKind::Runtime, "fn.list"),
        ("(match 1 (2 0))", ErrorKind::Runtime, "expr.match"),
        ("Point{x: 1}.y", ErrorKind::Runtime, "expr.field"),
        ("(with 1 {x: 2})", ErrorKind::Runtime, "expr.with"),
    ] {
        let err = eval(source).unwrap_err();
        assert_eq!(err.kind, kind, "{source}: {err}");
        assert_eq!(err.rule(), rule, "{source}: {err}");
        assert_eq!(
            err.to_string().lines().next().unwrap(),
            format!("{}: {} [{rule}]", err.at, err.message)
        );
    }
}

#[test]
fn language_and_schema_nesting() {
    let source = format!("{}0{}", "[".repeat(256), "]".repeat(256));
    assert_eq!(eval(&source).unwrap_err().rule(), "src.nesting-limit");
    let source = format!("{}i64{}", "(list ".repeat(256), ")".repeat(256));
    assert_eq!(
        Schema::parse(&source).unwrap_err().rule(),
        "src.nesting-limit"
    );
}

#[test]
fn schema_diagnostics() {
    for (source, rule) in [
        ("", "schema.program"),
        ("i64 bool", "schema.program"),
        ("(list)", "schema.syntax"),
        ("Unknown", "schema.reference"),
        ("(tuple i64)", "schema.tuple"),
    ] {
        let err = Schema::parse(source).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
        assert_eq!(err.rule(), rule, "{source}: {err}");
    }
    for (schema, value, rule) in [
        ("i64", "true", "schema.type"),
        ("(tuple i64 i64)", "(tuple 1 2 3)", "schema.tuple"),
        ("(struct Point (x i64))", "Point{}", "schema.struct"),
        ("(variant Color (Red))", "Color.Blue", "schema.variant"),
        ("(variant Color (Red))", "Color.Red(1)", "schema.variant"),
        (
            "(defschema A B) (defschema B A) A",
            "1",
            "schema.productive",
        ),
    ] {
        let err = Schema::parse(schema)
            .unwrap()
            .check(&eval(value).unwrap())
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Runtime);
        assert_eq!(err.rule(), rule, "{schema}: {err}");
        assert_eq!(err.at, rin::Span::default());
    }
}
