pub mod alloc;
mod ast;
mod bytecode;
mod compiler;
pub mod error;
mod eval;
pub mod ffi;
mod lexer;
mod parser;
mod pool;
pub mod schema;
pub mod value;

pub use alloc::{Allocator, ArenaAllocator, HeapAllocator};
pub use error::{Error, ErrorKind, Limits, Span};
pub use eval::{Machine, eval_with, eval_with_allocator};
pub use schema::Schema;
pub use value::Value;

/// Evaluates Rin source code with default limits and no external bindings.
pub fn eval(source: &str) -> Result<Value, Error> {
    eval_with(source, &[], Limits::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn arithmetic_and_comptime_sine_table() {
        let src = r#"
            (map (fn (i) (sin (/ (* 2.0 pi (as_f64 i)) 4.0))) (range 0 4))
        "#;
        let Value::List(xs) = eval(src).unwrap() else {
            panic!("expected list");
        };
        assert_eq!(xs.len(), 4);
        let Some(Value::Float(v1)) = xs.get(1) else {
            panic!("expected float");
        };
        assert!((v1 - 1.0).abs() < 1e-12);
    }

    #[test]
    fn no_implicit_int_float_mix() {
        let err = eval("(+ 1 1.0)").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Runtime);
    }

    #[test]
    fn struct_field_index_with_and_match() {
        let mut foe = BTreeMap::new();
        foe.insert("hp".into(), Value::Int(20));
        foe.insert("element".into(), Value::variant("Element", "Frost", None));

        let mut battle = BTreeMap::new();
        battle.insert(
            "foes".into(),
            Value::list(vec![Value::structure("Foe", foe)]),
        );

        let src = r#"
            (defn calc (b target)
              (match b.foes[target].element
                (Element.Frost [(with b.foes[target] { hp: (- b.foes[target].hp 9) })])
                (Element.Fire  [(with b.foes[target] { hp: (- b.foes[target].hp 3) })])
                (_             [(with b.foes[target] { hp: (- b.foes[target].hp 6) })])))
            (calc battle 0)
        "#;
        let val = eval_with(
            src,
            &[("battle", Value::structure("BattleState", battle))],
            Limits::default(),
        )
        .unwrap();

        let Value::List(xs) = val else {
            panic!("expected list");
        };
        let Some(Value::Struct { fields, .. }) = xs.get(0) else {
            panic!("expected struct");
        };
        assert_eq!(fields.get("hp"), Some(&Value::Int(11)));
    }

    #[test]
    fn recursive_defn_and_step_limit() {
        let src = r#"
            (defn loop_forever (n) (loop_forever (+ n 1)))
            (loop_forever 0)
        "#;
        let err = eval_with(
            src,
            &[],
            Limits {
                steps: 50,
                depth: 200,
                memory_bytes: 1024 * 1024,
            },
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::TooManySteps);
    }

    #[test]
    fn step_limit_error_reports_exact_span() {
        let limits = |steps| Limits {
            steps,
            depth: 200,
            memory_bytes: 1024 * 1024,
        };
        let e1 = eval_with("(let (x 1) [x (/ 1 0)])", &[], limits(1)).unwrap_err();
        assert_eq!(e1.kind, ErrorKind::TooManySteps);
        assert_eq!((e1.at.line, e1.at.col), (1, 9));

        let e2 = eval_with("(+ 1\n   zzz)", &[], limits(2)).unwrap_err();
        assert_eq!(e2.kind, ErrorKind::TooManySteps);
        assert_eq!((e2.at.line, e2.at.col), (1, 4));

        let e3 = eval_with("(def a 1) (def a (+ a 1)) a", &[], limits(1)).unwrap_err();
        assert_eq!(e3.kind, ErrorKind::TooManySteps);
        assert_eq!((e3.at.line, e3.at.col), (1, 8));
    }

    #[test]
    fn list_vs_index_syntax() {
        let src = r#"
            (let (xs [10, 20, 30])
              (+ xs[0] xs[(+ 1 1)]))
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(40));
    }

    #[test]
    fn higher_order_fold_and_filter() {
        let src = r#"
            (let (evens (filter (fn (x) (== (% x 2) 0)) (range 0 6)))
              (fold (fn (acc x) (+ acc x)) 0 evens))
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(6));
    }

    #[test]
    fn lisp_list_operations() {
        assert_eq!(eval("(is_empty [])").unwrap(), Value::Bool(true));
        assert_eq!(eval("(cons 1 [2, 3])").unwrap(), eval("[1, 2, 3]").unwrap());
        assert_eq!(eval("(push [1, 2] 3)").unwrap(), eval("[1, 2, 3]").unwrap());
        assert_eq!(
            eval("(append [1] [2, 3] [4])").unwrap(),
            eval("[1, 2, 3, 4]").unwrap()
        );
        assert_eq!(eval("(car [10, 20])").unwrap(), eval("some(10)").unwrap());
        assert_eq!(eval("(car [])").unwrap(), Value::None);
        assert_eq!(
            eval("(cdr [10, 20, 30])").unwrap(),
            eval("[20, 30]").unwrap()
        );
        assert_eq!(eval("(cdr [])").unwrap(), eval("[]").unwrap());
        assert_eq!(eval("(last [10, 20])").unwrap(), eval("some(20)").unwrap());
        assert_eq!(eval("(get [10, 20] 5)").unwrap(), Value::None);
        assert_eq!(
            eval("(reverse [1, 2, 3])").unwrap(),
            eval("[3, 2, 1]").unwrap()
        );
        assert_eq!(
            eval("(take [1, 2, 3, 4] 2)").unwrap(),
            eval("[1, 2]").unwrap()
        );
        assert_eq!(
            eval("(drop [1, 2, 3, 4] 2)").unwrap(),
            eval("[3, 4]").unwrap()
        );
        assert_eq!(
            eval("(slice [10, 20, 30, 40] 1 3)").unwrap(),
            eval("[20, 30]").unwrap()
        );
        assert_eq!(
            eval("(flatten [[1, 2], [3]])").unwrap(),
            eval("[1, 2, 3]").unwrap()
        );
        assert_eq!(
            eval("(flat_map (fn (x) [x, (* x 10)]) [1, 2])").unwrap(),
            eval("[1, 10, 2, 20]").unwrap()
        );
        assert_eq!(eval("(contains [1, 2, 3] 2)").unwrap(), Value::Bool(true));
        assert_eq!(
            eval("(find (fn (x) (> x 3)) [1, 4, 5])").unwrap(),
            eval("some(4)").unwrap()
        );
        assert_eq!(
            eval("(any (fn (x) (> x 3)) [1, 4])").unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            eval("(all (fn (x) (> x 0)) [1, 2])").unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            eval("(zip [1, 2] [\"a\", \"b\", \"c\"])").unwrap(),
            eval("[(1, \"a\"), (2, \"b\")]").unwrap()
        );
        assert_eq!(
            eval("(enumerate [\"a\", \"b\"])").unwrap(),
            eval("[(0, \"a\"), (1, \"b\")]").unwrap()
        );
        assert_eq!(
            eval("(sort [3, 1, 2])").unwrap(),
            eval("[1, 2, 3]").unwrap()
        );
        assert_eq!(
            eval("(sort_by (fn (x) (- 0 x)) [1, 3, 2])").unwrap(),
            eval("[3, 2, 1]").unwrap()
        );
        assert_eq!(
            eval("(unique [1, 2, 1, 3, 2])").unwrap(),
            eval("[1, 2, 3]").unwrap()
        );
        assert_eq!(eval("(list 1 2 3)").unwrap(), eval("[1, 2, 3]").unwrap());
        let rec_map = r#"
            (defn my_map (f xs)
              (match (car xs)
                (none [])
                (some(h) (cons (f h) (my_map f (cdr xs))))))
            (my_map (fn (x) (* x 2)) (list 1 2 3))
        "#;
        assert_eq!(eval(rec_map).unwrap(), eval("(list 2 4 6)").unwrap());
    }

    #[test]
    fn arena_allocator_reuses_slots_across_many_calls() {
        let mut backing = vec![0u8; 64 * 1024];
        let mut arena = ArenaAllocator::new(&mut backing);
        {
            let mut machine = Machine::new(Limits::default(), &mut arena);
            machine
                .eval("(defn sum_double (n) (fold + 0 (map (fn (x) (* x 2)) (range 0 n))))")
                .unwrap();
            let baseline = machine.live_bytes();

            for _ in 0..500 {
                let res = machine.eval("(sum_double 50)").unwrap();
                assert_eq!(res, Value::Int(2450));
                assert_eq!(machine.live_bytes(), baseline);
            }
        }
        assert!(arena.used() > 0 && arena.used() <= 64 * 1024);
    }

    #[test]
    fn arena_exhaustion_returns_too_much_memory() {
        let mut tiny = vec![0u8; 256];
        let mut arena = ArenaAllocator::new(&mut tiny);
        let err =
            eval_with_allocator("(range 0 20)", &[], Limits::default(), &mut arena).unwrap_err();
        assert_eq!(err.kind, ErrorKind::TooMuchMemory);
    }

    #[test]
    fn list_array_conversion_and_indexing() {
        let src = r#"
            (let (arr (list->array (list 10 20 30)))
              (+ arr[0] (len arr) (match (get arr 2) (some(v) v) (none 0))))
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(43));
        assert_eq!(
            eval("(array->list (list->array [1, 2, 3]))").unwrap(),
            eval("(list 1 2 3)").unwrap()
        );
    }

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

        let ok_val = eval(r#"[Card { id: "strike", cost: 1, element: Element.Fire }]"#).unwrap();
        assert!(s.check(&ok_val).is_ok());

        let extra_field =
            eval(r#"[Card { id: "strike", cost: 1, element: Element.Fire, rarity: 2 }]"#).unwrap();
        assert!(s.check(&extra_field).is_err());

        let open_s =
            Schema::parse("(list (open_struct Card (id string) (cost i64) (element any)))")
                .unwrap();
        assert!(open_s.check(&extra_field).is_ok());

        let rec = Schema::parse(
            r#"
            (defschema Node (struct Node (val i64) (next (option Node))))
            Node
            "#,
        )
        .unwrap();
        let tree = eval("Node { val: 1, next: some(Node { val: 2, next: none }) }").unwrap();
        assert!(rec.check(&tree).is_ok());

        unsafe {
            let v = ffi::rin_i64s([1, 2, 3].as_ptr(), 3);
            let sc_src = CString::new("(array i64)").unwrap();
            let bad_src = CString::new("(list i64)").unwrap();
            let sc = ffi::rin_schema(sc_src.as_ptr());
            assert!(!sc.is_null());
            assert!(ffi::rin_check(v, sc));
            assert!(ffi::rin_check_str(v, sc_src.as_ptr()));
            assert!(!ffi::rin_check_str(v, bad_src.as_ptr()));
            ffi::rin_schema_free(sc);
            ffi::rin_drop(v);
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

    #[test]
    fn modules_import_export_isolation_and_ffi_call_in() {
        use std::ffi::CString;

        let mut alloc = HeapAllocator;
        let mut machine = Machine::new(Limits::default(), &mut alloc);
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

        // Private helper inside `base` is not exported or leaked
        assert!(machine.eval("(import base) (base.helper 1)").is_err());
        assert!(machine.eval("(helper 1)").is_err());

        // `ember.on_play` calls `base.deal_damage`, which calls `base`'s private `helper`
        let res = machine
            .call_in("ember", "on_play", &[Value::Int(1)])
            .unwrap();
        let schema =
            Schema::parse("(list (struct Action (kind string) (target i64) (amount i64)))")
                .unwrap();
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
            let mut cr =
                ffi::rin_call_in(m, mod_base.as_ptr(), fn_double.as_ptr(), args.as_ptr(), 1);
            ffi::rin_drop(arg);
            assert!(cr.ok);
            let mut out = 0;
            assert!(ffi::rin_as_int(cr.value, &mut out));
            assert_eq!(out, 42);
            ffi::rin_res_free(&mut cr);
            ffi::rin_free(m);
        }
    }

    #[test]
    fn error_paths_do_not_leak_pool_slots() {
        let mut backing = vec![0u8; 64 * 1024];
        let mut arena = ArenaAllocator::new(&mut backing);
        let mut machine = Machine::new(
            Limits {
                steps: 200,
                depth: 50,
                memory_bytes: 64 * 1024,
            },
            &mut arena,
        );
        machine
            .eval(
                r#"
                (defn boom (n)
                  (let (tmp (range 0 10))
                    (if (== n 0) (/ tmp[0] 0) (boom (- n 1)))))
                (defn loop_step (n)
                  (let (tmp [1, 2, 3])
                    (loop_step (+ n (len tmp)))))
                "#,
            )
            .unwrap();
        let baseline = machine.live_bytes();

        for _ in 0..20 {
            let err = machine.eval("(boom 3)").unwrap_err();
            assert_eq!(err.kind, ErrorKind::Runtime);
            assert_eq!(machine.live_bytes(), baseline);

            let step_err = machine.eval("(loop_step 0)").unwrap_err();
            assert_eq!(step_err.kind, ErrorKind::TooManySteps);
            assert_eq!(machine.live_bytes(), baseline);

            let with_err = machine
                .eval("(with Card { cost: 1 } { missing: 2 })")
                .unwrap_err();
            assert_eq!(with_err.kind, ErrorKind::Runtime);
            assert_eq!(machine.live_bytes(), baseline);
        }
    }

    #[test]
    fn nested_upvalue_closures_and_shadowing() {
        let src = r#"
            (let (a 10)
              (let (f1 (fn (b)
                         (let (c 5)
                           (fn (d) (+ a b c d)))))
                (let (a 999)
                  ((f1 20) 7))))
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(42));
    }

    #[test]
    fn edge_cases_short_circuit_overflow_when_empty_match() {
        assert_eq!(eval("").unwrap(), Value::Void);
        assert_eq!(eval("(when false (/ 1 0))").unwrap(), Value::Void);
        assert_eq!(
            eval("(and false (== (/ 1 0) 0))").unwrap(),
            Value::Bool(false)
        );
        assert_eq!(eval("(or true (== (/ 1 0) 0))").unwrap(), Value::Bool(true));

        let overflow = eval("(+ 9223372036854775807 1)").unwrap_err();
        assert_eq!(overflow.kind, ErrorKind::Runtime);

        let div_overflow = eval("(/ -9223372036854775808 -1)").unwrap_err();
        assert_eq!(div_overflow.kind, ErrorKind::Runtime);

        let non_exhaustive = eval("(match 3 (1 10) (2 20))").unwrap_err();
        assert_eq!(non_exhaustive.kind, ErrorKind::Runtime);
    }

    #[test]
    fn tail_call_optimization_and_with_eval_order() {
        let mut alloc = HeapAllocator;
        let mut m = Machine::new(
            Limits {
                steps: 100_000,
                depth: 10,
                memory_bytes: 16 * 1024 * 1024,
            },
            &mut alloc,
        );
        // Depth is 10, yet 500 tail-recursive calls succeed without TooDeep
        let sum = m
            .eval(
                r#"
                (defn tail_sum (n acc)
                  (if (== n 0) acc (tail_sum (- n 1) (+ acc n))))
                (tail_sum 500 0)
                "#,
            )
            .unwrap();
        assert_eq!(sum, Value::Int(125_250));

        // `with` checks struct type first, then each field existence before evaluating value
        let err1 = eval("(with 1 { a: nosuch })").unwrap_err();
        assert!(err1.message.contains("requires a struct"), "{err1}");

        let err2 = eval("(with Card { cost: 1 } { missing: (/ 1 0) })").unwrap_err();
        assert!(err2.message.contains("no field 'missing'"), "{err2}");
    }

    #[test]
    fn memory_limit_does_not_double_free_shared_values_or_modules() {
        // Repro 1: sharing a global string in a list that exceeds memory_bytes
        let mut alloc = HeapAllocator;
        let mut m = Machine::new(
            Limits {
                steps: 100_000,
                depth: 200,
                memory_bytes: 4000,
            },
            &mut alloc,
        );
        m.eval(r#"(def s "hello")"#).unwrap();
        let big_list = format!("[{}]", vec!["s"; 200].join(" "));
        let err = m.eval(&big_list).unwrap_err();
        assert_eq!(err.kind, ErrorKind::TooMuchMemory);
        assert_eq!(m.eval("s").unwrap(), Value::str("hello"));

        // Repro 2: load_module failing at memory_bytes limit must not corrupt globals
        let mut alloc2 = HeapAllocator;
        let mut m2 = Machine::new(
            Limits {
                steps: 100_000,
                depth: 200,
                memory_bytes: 100,
            },
            &mut alloc2,
        );
        let merr = m2.load_module("m", r#"(def s "abc")"#).unwrap_err();
        assert_eq!(merr.kind, ErrorKind::TooMuchMemory);
        m2.eval(r#"(def t "xyz")"#).unwrap();
        assert_eq!(m2.eval("t").unwrap(), Value::str("xyz"));
    }

    #[test]
    fn cross_function_tail_calls_and_first_class_and_or() {
        let src = r#"
            (defn g (x) (let (a 1 b 2) (+ x a b)))
            (defn f (x) (g x))
            (defn h () (g 10))
            (+ (f 1) (h))
        "#;
        assert_eq!(eval(src).unwrap(), Value::Int(17));

        let diff_arity = r#"
            (defn g3 (x y z) (+ x y z))
            (defn f0 () (g3 1 2 3))
            (f0)
        "#;
        assert_eq!(eval(diff_arity).unwrap(), Value::Int(6));

        let mut alloc = HeapAllocator;
        let mut m = Machine::new(Limits::default(), &mut alloc);
        m.load_module(
            "m",
            r#"
            (defn helper (x) (let (k 10) (+ x k)))
            (defn entry (x) (helper x))
            "#,
        )
        .unwrap();
        assert_eq!(
            m.call_in("m", "entry", &[Value::Int(5)]).unwrap(),
            Value::Int(15)
        );

        assert_eq!(
            eval("(let (f and) (f true false))").unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            eval("(let (f or) (f false true))").unwrap(),
            Value::Bool(true)
        );
    }
}
