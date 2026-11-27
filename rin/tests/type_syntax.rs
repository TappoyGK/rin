#![cfg(feature = "std-value")]

use rin::ffi::RinErrorKind;
use rin::{Builder, Error, ErrorKind, HeapAllocator, Limits, Span, Value, eval};

#[test]
fn error_kind_type_and_ffi_mapping() {
    let err = Error::type_err("type.static", Span::new(1, 1), "type mismatch");
    assert_eq!(err.kind, ErrorKind::Type);
    assert_eq!(err.rule(), "type.static");
    assert_eq!(RinErrorKind::from(ErrorKind::Type), RinErrorKind::Type);
    assert_eq!(RinErrorKind::Type as u32, 7);
}

#[test]
fn defschema_syntax_and_placement() {
    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);
    b.load_module("game", "(defschema HitCtx (struct HitCtx (atk i64)))")
        .unwrap();
    let src = r#"
(import game)
(defschema Element (variant Element (Plain) (Fire) (Frost) (Custom text)))
(defschema HitCtx (struct HitCtx (atk i64) (element Element)))
(defschema OpenCtx (open_struct OpenCtx (id i64) (tags (list text))))
(defschema CopyCtx HitCtx)
(defschema ModCopy game.HitCtx)
(defschema Complex (struct Complex
  (cb (fn ((list i64) text) (result (tuple i64 bool) (option datum))))
  (arr (array f64))
  (ch char)
  (sym ident)
  (u void)))
1
"#;
    assert_eq!(b.eval(src).unwrap(), Value::Int(1));

    // 小文字始まりのスキーマ名は構文エラー
    let err = eval("(defschema foo (struct Foo (x i64)))").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "type.defschema");

    // `any` は型式として禁止
    let err = eval("(defschema Bad (struct Bad (x any)))").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "type.expr");

    // 1 要素タプル型は構文エラー
    let err = eval("(defschema Bad (struct Bad (x (tuple i64))))").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "schema.tuple");

    // 式の中の defschema は構文エラー
    let err = eval("(defn f () -> i64 (defschema Inner (struct Inner (x i64))) 1)").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "type.defschema");

    // 凍結後の defschema は env.frozen 構文エラー
    let mut alloc = HeapAllocator;
    let b = Builder::new(Limits::default(), &mut alloc);
    let mut m = b.freeze().unwrap();
    let err = m
        .eval("(defschema Frozen (struct Frozen (x i64)))")
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "env.frozen");
}

#[test]
fn typed_defn_fn_and_def_syntax_and_eval() {
    let src = r#"
(def x i64 10)
(def xs (list i64) [1 2 3])
(def plain (list 4 5))
(def pred (fn (i64) bool) (fn ((n i64)) -> bool (> n 0)))
(def ho (fn ((fn (i64) bool)) i64) (fn (f) (if (f 1) 100 0)))
(defn add ((a i64) (b i64)) -> i64 (+ a b))
(tuple x (len xs) (len plain) (ho pred) (add 3 4))
"#;
    assert_eq!(
        eval(src).unwrap(),
        Value::tuple(vec![
            Value::Int(10),
            Value::Int(3),
            Value::Int(2),
            Value::Int(100),
            Value::Int(7),
        ])
    );

    // 型注釈のない defn は静的型エラー
    let err = eval("(defn untyped (a b) (+ a b))").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Type);
    assert_eq!(err.rule(), "expr.defn");
}

#[test]
fn datum_type_patterns_and_module_variant_pattern() {
    let src = r#"
(defn classify ((d datum)) -> i64
  (match d
    (i64(n) (+ n 1))
    (f64(_) 2)
    (bool(b) (if b 3 4))
    (char(_) 5)
    (text(_) 6)
    (ident(_) 7)
    (list(xs) (+ 100 (len xs)))
    (none 0)))
[(classify 10) (classify 1.5) (classify true) (classify false)
 (classify \a) (classify "hi") (classify 'sym) (classify '(a 1 2)) (classify none)]
"#;
    assert_eq!(
        eval(src).unwrap(),
        Value::list(vec![
            Value::Int(11),
            Value::Int(2),
            Value::Int(3),
            Value::Int(4),
            Value::Int(5),
            Value::Int(6),
            Value::Int(7),
            Value::Int(103),
            Value::Int(0),
        ])
    );

    // モジュール修飾バリアントパターンが型検査・実行できる
    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);
    b.load_module(
        "game",
        "(defschema Element (variant Element (Plain) (Fire))) (defn fire () -> Element Element.Fire)",
    )
    .unwrap();
    let src_mod_pat = r#"
(import game)
(match (game.fire)
  (game.Element.Fire 1)
  (game.Element.Plain 2))
"#;
    assert_eq!(b.eval(src_mod_pat).unwrap(), Value::Int(1));
}

#[test]
fn host_boundary_type_validation() {
    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);
    b.eval("(defschema Ctx (struct Ctx (atk i64)))").unwrap();
    b.eval("(defn dmg ((ctx Ctx) (mult i64)) -> i64 (* ctx.atk mult))")
        .unwrap();

    // bind の型不一致は type.boundary
    let err = b.bind("bad", "i64", &Value::text("oops")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.rule(), "type.boundary");

    // 正常な bind
    b.bind("base", "i64", &Value::Int(10)).unwrap();

    let mut m = b.freeze().unwrap();
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("atk".to_string(), Value::Int(7));
    let ctx = Value::structure("Ctx", fields);
    assert_eq!(
        m.call("dmg", &[ctx.clone(), Value::Int(3)]).unwrap(),
        Value::Int(21)
    );

    // call の引数型不一致は type.boundary
    let err = m.call("dmg", &[Value::Int(7), Value::Int(3)]).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.rule(), "type.boundary");

    // eval_with の値型不一致は type.boundary
    let err = m
        .eval_with("(+ x base)", &[("x", "i64", Value::Bool(true))])
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.rule(), "type.boundary");

    // eval_with の正常動作（永続 TypeEnv を汚染しない）
    assert_eq!(
        m.eval_with("(+ x base)", &[("x", "i64", Value::Int(5))])
            .unwrap(),
        Value::Int(15)
    );
    assert_eq!(m.eval("x").unwrap_err().kind, ErrorKind::Type);
}

#[test]
fn fn_sig_query_and_preresolved_type_handle_rust_and_ffi() {
    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);
    b.load_module(
        "combat",
        r#"
(defschema Ctx (struct Ctx (atk i64) (crit bool)))
(defn calc ((ctx Ctx) (bonus i64)) -> i64 (if ctx.crit (* ctx.atk 2) (+ ctx.atk bonus)))
(def version 3)
"#,
    )
    .unwrap();
    b.eval(
        r#"
(import combat)
(defschema Wrap (struct Wrap (ctx combat.Ctx)))
(defn entry ((w Wrap)) -> (option i64) some(w.ctx.atk))
"#,
    )
    .unwrap();

    // 事前解決 TypeHandle（有効・無効・fn 型拒否）
    let ty_i64 = b.resolve_type("i64").unwrap();
    let ty_ctx = b.resolve_type("combat.Ctx").unwrap();
    assert_eq!(
        b.resolve_type("(fn (i64) i64)").unwrap_err().rule(),
        "type.boundary"
    );
    let v10 = b.val_int(10);
    b.bind_typed("k", ty_i64, v10).unwrap();
    b.release_val(v10);

    // Builder 段階でのシグネチャ・型照会
    let sig_mod = b.fn_sig_in("combat", "calc").unwrap();
    assert_eq!(sig_mod.param_count(), 2);
    assert_eq!(sig_mod.param_name(0), Some("ctx"));
    assert_eq!(sig_mod.param_type(0).unwrap().to_string(), "combat.Ctx");
    assert_eq!(sig_mod.param_name(1), Some("bonus"));
    assert_eq!(sig_mod.param_type(1).unwrap().to_string(), "i64");
    assert_eq!(sig_mod.ret_type().to_string(), "i64");
    assert_eq!(sig_mod.to_string(), "((ctx combat.Ctx) (bonus i64)) -> i64");
    assert_eq!(
        b.type_of_in("combat", "version").unwrap().to_string(),
        "i64"
    );
    assert!(b.fn_sig_in("combat", "version").is_none());

    let mut m = b.freeze().unwrap();

    // Machine 段階でのシグネチャ・型照会
    let sig_root = m.fn_sig("entry").unwrap();
    assert_eq!(sig_root.param_count(), 1);
    assert_eq!(sig_root.param_name(0), Some("w"));
    assert_eq!(sig_root.param_type(0).unwrap().to_string(), "Wrap");
    assert_eq!(sig_root.ret_type().to_string(), "(option i64)");
    assert_eq!(sig_root.to_string(), "((w Wrap)) -> (option i64)");
    assert_eq!(m.type_of("k").unwrap().to_string(), "i64");

    // 事前解決 TypeHandle を使った反復 eval_with_typed / eval_with_typed_vals
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("atk".to_string(), Value::Int(12));
    fields.insert("crit".to_string(), Value::Bool(true));
    let ctx_val = Value::structure("Ctx", fields);
    assert_eq!(
        m.eval_with_typed(
            "(+ (combat.calc c k) n)",
            &[("c", ty_ctx, ctx_val.clone()), ("n", ty_i64, Value::Int(5))],
        )
        .unwrap(),
        Value::Int(29)
    );
    // 型不一致は type.boundary
    let err = m
        .eval_with_typed("(+ n k)", &[("n", ty_i64, Value::text("bad"))])
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.rule(), "type.boundary");

    // C FFI での rin_resolve_type / rin_bind_typed / rin_eval_with_typed / rin_fn_arity / rin_fn_sig_format
    use rin::ffi::*;
    unsafe {
        let r = rin_new(
            rin_limits_default(),
            RinAllocator {
                ctx: std::ptr::null_mut(),
                alloc: None,
                dealloc: None,
            },
        );
        assert!(
            rin_load(
                r,
                c"m".as_ptr(),
                c"(defn add ((a i64) (b i64)) -> i64 (+ a b))".as_ptr()
            )
            .ok
        );
        let mut ty = RinType { id: 0 };
        assert!(rin_resolve_type(r, c"i64".as_ptr(), &mut ty));
        let v7 = rin_int(r, 7);
        assert!(rin_bind_typed(r, c"base".as_ptr(), ty, v7));
        rin_drop(r, v7);

        let mut arity = 0usize;
        assert!(rin_fn_arity_in(
            r,
            c"m".as_ptr(),
            c"add".as_ptr(),
            &mut arity
        ));
        assert_eq!(arity, 2);
        let mut buf = [0u8; 64];
        let needed = rin_fn_sig_format_in(
            r,
            c"m".as_ptr(),
            c"add".as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
        );
        assert!(needed > 0 && needed < buf.len());
        let sig_c = std::ffi::CStr::from_ptr(buf.as_ptr().cast())
            .to_str()
            .unwrap();
        assert_eq!(sig_c, "((a i64) (b i64)) -> i64");

        assert!(rin_freeze(r));
        let vx = rin_int(r, 3);
        let binding = RinTypedBinding {
            name: c"x".as_ptr(),
            ty,
            value: vx,
        };
        let res = rin_eval_with_typed(r, c"(m.add base x)".as_ptr(), &binding, 1);
        assert!(res.ok);
        let mut out_i = 0i64;
        assert!(rin_as_int(r, res.value, &mut out_i));
        assert_eq!(out_i, 10);
        rin_drop(r, vx);
        rin_free(r);
    }
}

#[test]
fn type_expr_nesting_limit_enforced() {
    let mut src = String::from("(defschema Deep (struct Deep (x ");
    for _ in 0..260 {
        src.push_str("(list ");
    }
    src.push_str("i64");
    for _ in 0..260 {
        src.push(')');
    }
    src.push_str(")))");
    let err = eval(&src).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "src.nesting-limit");
}

#[test]
fn formatter_handles_defschema_type_annotations_and_datum_patterns() {
    let input = "(defschema Element (variant Element (Plain) (Fire) (Frost)))\n(defschema HitCtx (struct HitCtx (atk i64) (element Element)))\n(def limit i64 10)\n(defn on-hit ((ctx HitCtx)) -> i64 (match ctx.element (game.Element.Fire (* ctx.atk 2)) (_ ctx.atk)))\n(defn sum-ints ((d datum)) -> i64 (match d (i64(n) n) (list(xs) (fold (fn ((acc i64) (x datum)) -> i64 (+ acc (sum-ints x))) 0 xs)) (_ 0)))\n";
    let formatted = rin::fmt::format(input).expect("format should succeed");
    // 冪等性
    let formatted_again = rin::fmt::format(&formatted).expect("second format should succeed");
    assert_eq!(formatted, formatted_again);
    // 定義間に空行が入る
    assert!(formatted.contains(")\n\n(defschema HitCtx"));
    assert!(formatted.contains(")\n\n(def limit i64 10)"));
    assert!(formatted.contains(")\n\n(defn on-hit"));
    // 型パターン・モジュール修飾パターンの隣接関係が保たれる
    assert!(formatted.contains("i64(n)"));
    assert!(formatted.contains("list(xs)"));
    assert!(formatted.contains("game.Element.Fire"));
}
