use crate::{Builder, HeapAllocator, Limits, Schema, Value, ffi};

#[test]
fn modules_import_export_isolation_and_ffi_call_in() {
    use std::ffi::CString;

    let mut alloc = HeapAllocator;
    let mut machine = Builder::new(Limits::default(), &mut alloc);
    machine
        .load_module(
            "base",
            r#"
            (export deal_damage)
            (defn helper (n) (* n 2))
            (defn deal_damage (target amount)
              Action { kind: "damage" target: target amount: (helper amount) })
            "#,
        )
        .unwrap();
    machine
        .load_module(
            "ember",
            r#"
            (import base)
            (defn on_play (target)
              [(base.deal_damage target 5)])
            "#,
        )
        .unwrap();

    // `base` 内部の非公開関数 helper は外部に公開・漏洩しない。
    assert!(machine.eval("(import base) (base.helper 1)").is_err());
    assert!(machine.eval("(helper 1)").is_err());

    // `ember.on_play` は `base.deal_damage` 経由で `base` の非公開 `helper` を呼び出す。
    let res = machine
        .call_in("ember", "on_play", &[Value::Int(1)])
        .unwrap();
    let schema =
        Schema::parse("(list (struct Action (kind text) (target i64) (amount i64)))").unwrap();
    assert!(schema.check(&res).is_ok());

    let m = ffi::rin_new(
        ffi::rin_limits_default(),
        ffi::RinAllocator {
            ctx: std::ptr::null_mut(),
            alloc: None,
            dealloc: None,
        },
    );
    let mod_base = CString::new("base").unwrap();
    let src_base = CString::new("(defn double (x) (* x 2))").unwrap();
    let fn_double = CString::new("double").unwrap();
    unsafe {
        let lr = ffi::rin_load(m, mod_base.as_ptr(), src_base.as_ptr());
        assert!(lr.ok);

        let arg = ffi::rin_int(m, 21);
        let args = [arg as *const ffi::RinValue];
        let cr = ffi::rin_call_in(m, mod_base.as_ptr(), fn_double.as_ptr(), args.as_ptr(), 1);
        ffi::rin_drop(m, arg);
        assert!(cr.ok);
        let mut out = 0;
        assert!(ffi::rin_as_int(m, cr.value, &mut out));
        assert_eq!(out, 42);
        ffi::rin_free(m);
    }
}

#[test]
fn duplicate_exports_do_not_leak_on_reload_or_unknown_symbol_error() {
    let cases = [
        "(def x \"hello\") (export x)",
        "(def x \"hello\") (export x x)",
        "(def x \"hello\") (export x x x)",
        "(def x \"hello\") (export x) (export x)",
    ];

    let mut baseline = None;
    for src in cases {
        let mut alloc = HeapAllocator;
        let mut builder = Builder::new(Limits::default(), &mut alloc);
        builder.load_module("m", src).unwrap();
        let live = builder.live_bytes();
        // 成功済みモジュールの再読込はエラーになり、live_bytes も増えない。
        let err = builder.load_module("m", src).unwrap_err();
        assert_eq!(err.kind, crate::ErrorKind::Runtime);
        assert_eq!(err.rule(), "expr.module");
        assert_eq!(builder.live_bytes(), live);
        match baseline {
            None => baseline = Some(live),
            Some(expected) => assert_eq!(live, expected, "live_bytes differs from baseline: {src}"),
        }
        let val = builder.eval("(import m) m.x").unwrap();
        assert_eq!(val, Value::text("hello"));
    }

    // 複数シンボル混在の重複指定でも同じ公開値と生存バイト数になる。
    let mut alloc_single = HeapAllocator;
    let mut b_single = Builder::new(Limits::default(), &mut alloc_single);
    b_single
        .load_module("m", "(def a \"alpha\") (def b [1 2 3]) (export a b)")
        .unwrap();
    let mut alloc_dup = HeapAllocator;
    let mut b_dup = Builder::new(Limits::default(), &mut alloc_dup);
    b_dup
        .load_module(
            "m",
            "(def a \"alpha\") (def b [1 2 3]) (export b a b) (export a)",
        )
        .unwrap();
    assert_eq!(b_dup.live_bytes(), b_single.live_bytes());
    assert_eq!(
        b_dup.eval("(import m) (tuple m.a m.b)").unwrap(),
        b_single.eval("(import m) (tuple m.a m.b)").unwrap()
    );

    // 重複指定後に未定義の公開名で失敗しても値が残らない。
    for bad_src in [
        "(def x \"hello\") (export x x missing)",
        "(def x \"hello\") (export x) (export x missing)",
    ] {
        let mut alloc = HeapAllocator;
        let mut builder = Builder::new(Limits::default(), &mut alloc);
        let err = builder.load_module("m", bad_src).unwrap_err();
        assert_eq!(err.rule(), "expr.module");
        assert_eq!(builder.live_bytes(), 0, "leak after error: {bad_src}");
    }
}
