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
              Action { kind: "damage", target: target, amount: (helper amount) })
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
        Schema::parse("(list (struct Action (kind string) (target i64) (amount i64)))").unwrap();
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
        let mut lr = ffi::rin_load(m, mod_base.as_ptr(), src_base.as_ptr());
        assert!(lr.ok);
        ffi::rin_res_free(&mut lr);

        let arg = ffi::rin_int(21);
        let args = [arg as *const ffi::RinValue];
        let mut cr = ffi::rin_call_in(m, mod_base.as_ptr(), fn_double.as_ptr(), args.as_ptr(), 1);
        ffi::rin_drop(arg);
        assert!(cr.ok);
        let mut out = 0;
        assert!(ffi::rin_as_int(cr.value, &mut out));
        assert_eq!(out, 42);
        ffi::rin_res_free(&mut cr);
        ffi::rin_free(m);
    }
}
