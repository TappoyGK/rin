use rin::{Builder, ErrorKind, HeapAllocator, Limits, Value, eval};

fn hint(source: &str) -> String {
    let error = eval(source).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Runtime);
    error.message
}

#[test]
fn local_scope_and_closures() {
    for wrap_depth in [0, 12] {
        for (source, expected) in [
            (
                "(let (damage 1) damge)",
                "unknown name 'damge'; did you mean 'damage'?",
            ),
            (
                "((fn (damage) damge) 1)",
                "unknown name 'damge'; did you mean 'damage'?",
            ),
            (
                "(let (damage 1) ((fn () ((fn () damge)))))",
                "unknown name 'damge'; did you mean 'damage'?",
            ),
            (
                "(match 1 (damage damge))",
                "unknown name 'damge'; did you mean 'damage'?",
            ),
            ("(let (damage damge) damage)", "unknown name 'damge'"),
            ("(let (damage 1) 0) damge", "unknown name 'damge'"),
            (
                "(let (damage 1) 0) (let (health 2) damge)",
                "unknown name 'damge'",
            ),
            (
                "(match 2 (some(damage) 0) (_ damge))",
                "unknown name 'damge'",
            ),
            (
                "(defn broken () damge) (let (damage 1) (broken))",
                "unknown name 'damge'",
            ),
            (
                "(let (damage 1) (let (damage 2) damge))",
                "unknown name 'damge'; did you mean 'damage'?",
            ),
        ] {
            // 深い構文では明示的タスクスタックのコンパイル経路も検証する。
            let wrapped = format!(
                "{}{}{}",
                "(let () ".repeat(wrap_depth),
                source,
                ")".repeat(wrap_depth)
            );
            assert_eq!(hint(&wrapped), expected, "{wrapped}");
        }
    }
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(Limits::default(), &mut alloc);
    m.eval("(def saved (let (damage 1) (fn () damge)))")
        .unwrap();
    let mut m = m.freeze().unwrap();
    let err = m.call("saved", &[]).unwrap_err();
    assert_eq!(err.message, "unknown name 'damge'; did you mean 'damage'?");
    assert_eq!(err.rule(), "expr.name");
    assert!(err.to_string().contains("? [expr.name]\n  in "));
}

#[test]
fn globals_builtins_modules_and_private_names() {
    assert_eq!(
        hint("strt"),
        "unknown name 'strt'; did you mean 'sort', 'sqrt'?"
    );
    assert_eq!(hint("rang"), "unknown name 'rang'; did you mean 'range'?");
    assert_eq!(
        hint("(def damage 1) damge"),
        "unknown name 'damge'; did you mean 'damage'?"
    );
    assert_eq!(hint("damge (def damage 1)"), "unknown name 'damge'");
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(Limits::default(), &mut alloc);
    m.load_module(
        "combat",
        "(def damage 1) (defn attack () 1) (export attack)",
    )
    .unwrap();
    assert_eq!(m.eval("damge").unwrap_err().message, "unknown name 'damge'");
    // 未 import のモジュール名を通常の変数候補には含めない。
    assert_eq!(m.eval("combt").unwrap_err().message, "unknown name 'combt'");
    assert_eq!(
        m.eval("(import combt)").unwrap_err().message,
        "unknown module 'combt'; did you mean 'combat'?"
    );
    assert_eq!(
        m.eval("(import combat) combt").unwrap_err().message,
        "unknown name 'combt'; did you mean 'combat'?"
    );
    assert_eq!(
        m.eval("combat.damge").unwrap_err().message,
        "module 'combat' has no export 'damge'; exports: attack"
    );
    m.bind("damage", &Value::Int(1)).unwrap();
    m.load_module("other", "(def damage 2) (defn fail () damge)")
        .unwrap();
    m.load_module(
        "private",
        "(def shield 1) (defn fail () shild) (export fail)",
    )
    .unwrap();
    assert_eq!(m.eval("shild").unwrap_err().message, "unknown name 'shild'");
    m.bind("cat猫", &Value::Int(1)).unwrap();
    let mut m = m.freeze().unwrap();
    assert_eq!(
        m.call_in("combat", "damge", &[]).unwrap_err().message,
        "module 'combat' has no export 'damge'; exports: attack"
    );
    assert_eq!(
        m.call_in("combt", "attack", &[]).unwrap_err().message,
        "unknown module 'combt'; did you mean 'combat'?"
    );
    assert_eq!(
        m.call("damge", &[]).unwrap_err().message,
        "unknown function 'damge'; did you mean 'damage'?"
    );
    let err = m.call_in("other", "fail", &[]).unwrap_err();
    assert_eq!(err.message, "unknown name 'damge'; did you mean 'damage'?");
    assert_eq!(err.module(), Some("other"));
    assert_eq!(
        m.call_in("private", "fail", &[]).unwrap_err().message,
        "unknown name 'shild'; did you mean 'shield'?"
    );
    assert_eq!(
        m.call("cat犬", &[]).unwrap_err().message,
        "unknown function 'cat犬'; did you mean 'cat猫'?"
    );
    assert!(err.to_string().starts_with("other "));
}

#[test]
fn field_and_export_lists_are_sorted_and_bounded() {
    assert_eq!(
        hint("Point{z: 1, a: 2}.bad"),
        "Point has no field 'bad'; fields: a, z"
    );
    assert_eq!(
        hint("(with Point{z: 1, a: 2} {bad: 3})"),
        "Point has no field 'bad'; fields: a, z"
    );
    assert_eq!(
        hint("Point{}.bad"),
        "Point has no field 'bad'; fields: (none)"
    );
    let fields = (0..19)
        .rev()
        .map(|i| format!("f{i:02}: 0"))
        .collect::<Vec<_>>()
        .join(", ");
    let shown = (0..16)
        .map(|i| format!("f{i:02}"))
        .collect::<Vec<_>>()
        .join(", ");
    let expected = format!("Point has no field 'bad'; fields: {shown}, ... and 3 more");
    assert_eq!(hint(&format!("Point{{{fields}}}.bad")), expected);
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(Limits::default(), &mut alloc);
    m.load_module("empty", "(export)").unwrap();
    let defs = (0..19)
        .rev()
        .map(|i| format!("(def f{i:02} 0)"))
        .collect::<Vec<_>>()
        .join(" ");
    m.load_module("many", &defs).unwrap();
    let mut m = m.freeze().unwrap();
    assert_eq!(
        m.call_in("empty", "bad", &[]).unwrap_err().message,
        "module 'empty' has no export 'bad'; exports: (none)"
    );
    assert_eq!(
        m.call_in("many", "bad", &[]).unwrap_err().message,
        format!("module 'many' has no export 'bad'; exports: {shown}, ... and 3 more")
    );
}

#[test]
fn interned_names_and_reloaded_module_bindings_still_need_local_hints() {
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(Limits::default(), &mut alloc);
    // Interning an unexecuted definition does not make it a known global.
    for _ in 0..3 {
        let err = m
            .eval("(if false (def damge 1) 0) (let (damage 2) damge)")
            .unwrap_err();
        assert_eq!(err.message, "unknown name 'damge'; did you mean 'damage'?");
    }
    // Also compile while the previous module environment still contains damage.
    for _ in 0..2 {
        m.load_module(
            "combat",
            "(def damage 1) (def saved (let (damge 2) (fn () damage))) (export saved)",
        )
        .unwrap();
    }
    m.eval("(import combat) (def old combat.saved)").unwrap();
    assert_eq!(m.eval("(old)").unwrap(), Value::Int(1));
    // Old closures survive reloading, but their module's definitions can disappear.
    m.load_module("combat", "0").unwrap();
    assert_eq!(
        m.eval("(old)").unwrap_err().message,
        "unknown name 'damage'; did you mean 'damge'?"
    );
    m.bind("damage", &Value::Int(3)).unwrap();
    assert_eq!(m.eval("(old)").unwrap(), Value::Int(3));
    m.bind("damage", &Value::Int(4)).unwrap();
    assert_eq!(m.eval("(old)").unwrap(), Value::Int(4));
}
