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
