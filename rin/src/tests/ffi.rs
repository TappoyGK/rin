use crate::ffi;

#[test]
fn c_ffi_roundtrip_with_custom_allocator() {
    use std::ffi::{CString, c_void};

    struct BumpCtx {
        buf: Vec<u8>,
        offset: usize,
    }

    unsafe extern "C" fn c_alloc(ctx: *mut c_void, size: usize, align: usize) -> *mut u8 {
        if size == 0 {
            return std::ptr::NonNull::<u8>::dangling().as_ptr();
        }
        let bump = unsafe { &mut *(ctx as *mut BumpCtx) };
        let aligned = (bump.offset + align - 1) & !(align - 1);
        if aligned + size > bump.buf.len() {
            return std::ptr::null_mut();
        }
        bump.offset = aligned + size;
        unsafe { bump.buf.as_mut_ptr().add(aligned) }
    }

    let mut ctx = BumpCtx {
        buf: vec![0u8; 64 * 1024],
        offset: 0,
    };
    let alloc = ffi::RinAllocator {
        ctx: (&mut ctx as *mut BumpCtx).cast::<c_void>(),
        alloc: Some(c_alloc),
        dealloc: None,
    };
    let m = ffi::rin_new(ffi::rin_limits_default(), alloc);
    let input = [1i64, 2, 3, 4];
    let name = CString::new("xs").unwrap();
    let src = CString::new("(list->array (map (fn (x) (* x 10)) (array->list xs)))").unwrap();

    unsafe {
        let v_in = ffi::rin_i64s(input.as_ptr(), input.len());
        assert!(ffi::rin_bind(m, name.as_ptr(), v_in));
        ffi::rin_drop(v_in);

        let mut res = ffi::rin_eval(m, src.as_ptr());
        assert!(res.ok);
        assert_eq!(ffi::rin_tag(res.value), ffi::RinTag::Array);
        assert_eq!(ffi::rin_len(res.value), 4);

        let mut out = [0i64; 4];
        let copied = ffi::rin_copy_i64s(res.value, out.as_mut_ptr(), out.len());
        assert_eq!(copied, 4);
        assert_eq!(out, [10, 20, 30, 40]);

        ffi::rin_res_free(&mut res);
        ffi::rin_free(m);
    }
    assert!(ctx.offset > 0);
}

#[test]
fn c_ffi_list_library_roundtrip() {
    use std::ffi::CString;

    let m = ffi::rin_new(
        ffi::rin_limits_default(),
        ffi::RinAllocator {
            ctx: std::ptr::null_mut(),
            alloc: None,
            dealloc: None,
        },
    );
    let name = CString::new("xs").unwrap();
    let src = CString::new("(map (fn (x) (+ x 1)) xs)").unwrap();

    unsafe {
        let empty = ffi::rin_nil();
        let v20 = ffi::rin_int(20);
        let v10 = ffi::rin_int(10);
        let l1 = ffi::rin_cons(v20, empty);
        let l2 = ffi::rin_cons(v10, l1);
        assert!(ffi::rin_bind(m, name.as_ptr(), l2));

        ffi::rin_drop(empty);
        ffi::rin_drop(v20);
        ffi::rin_drop(v10);
        ffi::rin_drop(l1);
        ffi::rin_drop(l2);

        let mut res = ffi::rin_eval(m, src.as_ptr());
        assert!(res.ok);
        assert_eq!(ffi::rin_tag(res.value), ffi::RinTag::List);
        assert_eq!(ffi::rin_len(res.value), 2);

        let head = ffi::rin_car(res.value);
        let mut n = 0;
        assert!(ffi::rin_as_int(head, &mut n));
        assert_eq!(n, 11);
        ffi::rin_drop(head);

        let tail = ffi::rin_cdr(res.value);
        let head2 = ffi::rin_car(tail);
        assert!(ffi::rin_as_int(head2, &mut n));
        assert_eq!(n, 21);
        ffi::rin_drop(head2);
        ffi::rin_drop(tail);

        ffi::rin_res_free(&mut res);
        ffi::rin_free(m);
    }
}

#[test]
fn c_ffi_struct_variant_field_extraction() {
    use std::ffi::CString;

    let m = ffi::rin_new(
        ffi::rin_limits_default(),
        ffi::RinAllocator {
            ctx: std::ptr::null_mut(),
            alloc: None,
            dealloc: None,
        },
    );
    let src = CString::new(r#"Card { id: "strike", cost: 1, element: Element.Fire }"#).unwrap();
    let schema_src = CString::new(
        "(struct Card (id string) (cost i64) (element (variant Element (Plain) (Fire) (Frost))))",
    )
    .unwrap();
    let f_id = CString::new("id").unwrap();
    let f_cost = CString::new("cost").unwrap();
    let f_elem = CString::new("element").unwrap();

    unsafe {
        let mut res = ffi::rin_eval(m, src.as_ptr());
        assert!(res.ok);
        assert!(ffi::rin_check_str(res.value, schema_src.as_ptr()));

        let v_id = ffi::rin_field(res.value, f_id.as_ptr());
        let mut id_len = 0;
        let id_ptr = ffi::rin_as_str(v_id, &mut id_len);
        assert_eq!(std::slice::from_raw_parts(id_ptr, id_len), b"strike");
        ffi::rin_drop(v_id);

        let v_cost = ffi::rin_field(res.value, f_cost.as_ptr());
        let mut cost = 0;
        assert!(ffi::rin_as_int(v_cost, &mut cost));
        assert_eq!(cost, 1);
        ffi::rin_drop(v_cost);

        let v_elem = ffi::rin_field(res.value, f_elem.as_ptr());
        let mut vname_len = 0;
        let vname_ptr = ffi::rin_variant_name(v_elem, &mut vname_len);
        assert_eq!(std::slice::from_raw_parts(vname_ptr, vname_len), b"Fire");
        ffi::rin_drop(v_elem);

        ffi::rin_res_free(&mut res);
        ffi::rin_free(m);
    }
}

#[test]
fn c_ffi_call_and_struct_roundtrip() {
    use std::ffi::CString;

    let m = ffi::rin_new(
        ffi::rin_limits_default(),
        ffi::RinAllocator {
            ctx: std::ptr::null_mut(),
            alloc: None,
            dealloc: None,
        },
    );
    let def_src = CString::new(
        r#"
        (defn discount (card)
          (match card.element
            (Element.Frost (with card { cost: (- card.cost 1) }))
            (_ card)))
        "#,
    )
    .unwrap();
    let fn_name = CString::new("discount").unwrap();
    let t_card = CString::new("Card").unwrap();
    let f_id = CString::new("id").unwrap();
    let f_cost = CString::new("cost").unwrap();
    let f_elem = CString::new("element").unwrap();
    let v_id = CString::new("ice_lance").unwrap();
    let t_elem = CString::new("Element").unwrap();
    let v_frost = CString::new("Frost").unwrap();

    unsafe {
        let mut def_res = ffi::rin_eval(m, def_src.as_ptr());
        assert!(def_res.ok);
        ffi::rin_res_free(&mut def_res);

        let card = ffi::rin_struct(t_card.as_ptr());
        let id_val = ffi::rin_str(v_id.as_ptr());
        let cost_val = ffi::rin_int(3);
        let elem_val = ffi::rin_variant(t_elem.as_ptr(), v_frost.as_ptr(), std::ptr::null());
        assert!(ffi::rin_struct_set(card, f_id.as_ptr(), id_val));
        assert!(ffi::rin_struct_set(card, f_cost.as_ptr(), cost_val));
        assert!(ffi::rin_struct_set(card, f_elem.as_ptr(), elem_val));
        ffi::rin_drop(id_val);
        ffi::rin_drop(cost_val);
        ffi::rin_drop(elem_val);

        let args = [card as *const ffi::RinValue];
        let mut call_res = ffi::rin_call(m, fn_name.as_ptr(), args.as_ptr(), args.len());
        ffi::rin_drop(card);
        assert!(call_res.ok);

        let out_cost = ffi::rin_field(call_res.value, f_cost.as_ptr());
        let mut n = 0;
        assert!(ffi::rin_as_int(out_cost, &mut n));
        assert_eq!(n, 2);
        ffi::rin_drop(out_cost);

        ffi::rin_res_free(&mut call_res);
        ffi::rin_free(m);
    }
}
