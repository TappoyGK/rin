use rin::{Builder, ErrorKind, HeapAllocator, Limits, Value, eval};

fn hint(source: &str) -> String {
    let error = eval(source).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Type);
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
                "(match some(2) (some(damage) 0) (_ damge))",
                "unknown name 'damge'",
            ),
            (
                "(let (broken (fn () damge)) (let (damage 1) (broken)))",
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
    assert_eq!(
        hint("(defn broken () -> i64 damge) (let (damage 1) (broken))"),
        "unknown name 'damge'"
    );
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(Limits::default(), &mut alloc);
    let err = m
        .eval("(def saved (let (damage 1) (fn () damge)))")
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Type);
    assert_eq!(err.message, "unknown name 'damge'; did you mean 'damage'?");
    assert_eq!(err.rule(), "expr.name");
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
        "(def damage 1) (defn attack () -> i64 1) (export attack)",
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
    // 失敗した評価の (import combat) は取り消されているため、明示的に import してから参照する。
    m.eval("(import combat)").unwrap();
    assert_eq!(
        m.eval("combat.damge").unwrap_err().message,
        "module 'combat' has no export 'damge'; exports: attack"
    );
    m.bind("damage", "i64", &Value::Int(1)).unwrap();
    m.load_module("other", "(def damage 2) (defn fail () -> i64 damge)")
        .unwrap_err();
    m.load_module(
        "private",
        "(def shield 1) (defn fail () -> i64 shild) (export fail)",
    )
    .unwrap_err();
    assert_eq!(m.eval("shild").unwrap_err().message, "unknown name 'shild'");
    m.bind("cat猫", "i64", &Value::Int(1)).unwrap();
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
    assert_eq!(
        m.call("cat犬", &[]).unwrap_err().message,
        "unknown function 'cat犬'; did you mean 'cat猫'?"
    );
    // モジュール内の未定義名エラーでもモジュール内ローカル名ヒントとモジュール名が出る。
    let mut alloc2 = HeapAllocator;
    let mut m2 = Builder::new(Limits::default(), &mut alloc2);
    let err = m2
        .load_module("other", "(def damage 2) (defn fail () -> i64 damge)")
        .unwrap_err();
    assert_eq!(err.message, "unknown name 'damge'; did you mean 'damage'?");
    assert_eq!(err.module(), Some("other"));
    assert!(err.to_string().starts_with("other "));
    let err = m2
        .load_module(
            "private",
            "(def shield 1) (defn fail () -> i64 shild) (export fail)",
        )
        .unwrap_err();
    assert_eq!(err.message, "unknown name 'shild'; did you mean 'shield'?");
}

#[test]
fn field_and_export_lists_are_sorted_and_bounded() {
    assert_eq!(
        hint("(defschema Point (struct Point (z i64) (a i64))) Point{z: 1 a: 2}.bad"),
        "Point has no field 'bad'; fields: a, z"
    );
    assert_eq!(
        hint("(defschema Point (struct Point (z i64) (a i64))) (with Point{z: 1 a: 2} {bad: 3})"),
        "Point has no field 'bad'; fields: a, z"
    );
    assert_eq!(
        hint("(defschema Point (struct Point)) Point{}.bad"),
        "Point has no field 'bad'; fields: (none)"
    );
    let schema_fields = (0..19)
        .rev()
        .map(|i| format!("(f{i:02} i64)"))
        .collect::<Vec<_>>()
        .join(" ");
    let fields = (0..19)
        .rev()
        .map(|i| format!("f{i:02}: 0"))
        .collect::<Vec<_>>()
        .join(" ");
    let shown = (0..16)
        .map(|i| format!("f{i:02}"))
        .collect::<Vec<_>>()
        .join(", ");
    let expected = format!("Point has no field 'bad'; fields: {shown}, ... and 3 more");
    assert_eq!(
        hint(&format!(
            "(defschema Point (struct Point {schema_fields})) Point{{{fields}}}.bad"
        )),
        expected
    );
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
fn interned_names_from_failed_eval_do_not_become_global_hints() {
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(Limits::default(), &mut alloc);
    // Interning an unexecuted definition does not make it a known global.
    for _ in 0..3 {
        let err = m.eval("(let (damage 2) damge) (def damge 1)").unwrap_err();
        assert_eq!(err.message, "unknown name 'damge'; did you mean 'damage'?");
    }
    m.load_module(
        "combat",
        "(def damage 1) (def saved (let (damge 2) (fn () damage))) (export saved)",
    )
    .unwrap();
    // Reloading a successfully loaded module is rejected.
    assert_eq!(
        m.load_module("combat", "0").unwrap_err().rule(),
        "expr.module"
    );
    m.eval("(import combat) (def old combat.saved)").unwrap();
    assert_eq!(m.eval("(old)").unwrap(), Value::Int(1));
    m.bind("damage", "i64", &Value::Int(3)).unwrap();
    // Module closure keeps reading its own module's damage = 1.
    assert_eq!(m.eval("(old)").unwrap(), Value::Int(1));
    assert_eq!(
        m.bind("damage", "i64", &Value::Int(4)).unwrap_err().rule(),
        "expr.def"
    );
}
