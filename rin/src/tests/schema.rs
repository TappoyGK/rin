use crate::{Schema, eval, ffi};

#[test]
fn schema_validation_strict_open_recursive_and_ffi() {
    use std::ffi::CString;

    let s = Schema::parse(
        r#"
        (defschema Element (variant Element (Plain) (Fire) (Frost)))
        (defschema Card (struct Card (id string) (cost i64) (element Element)))
        (list Card)
        "#,
    )
    .unwrap();

    let ok_val = eval(r#"[Card { id: "strike" cost: 1 element: Element.Fire }]"#).unwrap();
    assert!(s.check(&ok_val).is_ok());

    let extra_field =
        eval(r#"[Card { id: "strike" cost: 1 element: Element.Fire rarity: 2 }]"#).unwrap();
    assert!(s.check(&extra_field).is_err());

    let open_s =
        Schema::parse("(list (open_struct Card (id string) (cost i64) (element any)))").unwrap();
    assert!(open_s.check(&extra_field).is_ok());

    let rec = Schema::parse(
        r#"
        (defschema Node (struct Node (val i64) (next (option Node))))
        Node
        "#,
    )
    .unwrap();
    let tree = eval("Node { val: 1 next: some(Node { val: 2 next: none }) }").unwrap();
    assert!(rec.check(&tree).is_ok());

    unsafe {
        let m = ffi::rin_new(
            ffi::rin_limits_default(),
            ffi::RinAllocator {
                ctx: std::ptr::null_mut(),
                alloc: None,
                dealloc: None,
            },
        );
        let v = ffi::rin_i64s(m, [1, 2, 3].as_ptr(), 3);
        let sc_src = CString::new("(array i64)").unwrap();
        let bad_src = CString::new("(list i64)").unwrap();
        let sc = ffi::rin_schema(sc_src.as_ptr());
        assert!(!sc.is_null());
        assert!(ffi::rin_check(m, v, sc));
        assert!(ffi::rin_check_str(m, v, sc_src.as_ptr()));
        assert!(!ffi::rin_check_str(m, v, bad_src.as_ptr()));
        ffi::rin_schema_free(sc);
        ffi::rin_drop(m, v);
        ffi::rin_free(m);
    }
}
