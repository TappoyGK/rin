//! Issue #80: `eval_with` のレキシカルスコープ化、`Builder` の同名再定義・モジュール再読込禁止、
//! および失敗時の束縛取り消しの網羅テスト。
#![cfg(feature = "std-value")]

use rin::{ArenaAllocator, Builder, ErrorKind, HeapAllocator, Limits, Value};

#[test]
fn eval_with_is_strictly_lexical_for_functions_and_closures() {
    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);
    b.eval(
        r#"
        (def x 1)
        (def cap (let (x 5) (fn () x)))
        (defn f () -> i64 (+ x 1))
        (defn invoke ((k (fn () i64))) -> i64 (k))
        (defn make () -> (fn () i64) (fn () x))
        "#,
    )
    .unwrap();
    let mut m = b.freeze().unwrap();
    let before = m.live_bytes();

    // Issue #80 確定仕様の境界例: (tuple 10 2 10 1) + 捕捉済みクロージャ 5
    let res = m
        .eval_with(
            "(tuple x (f) (invoke (fn () x)) ((make)) (cap))",
            &[("x", "i64", Value::Int(10))],
        )
        .unwrap();
    assert_eq!(
        res,
        Value::tuple(vec![
            Value::Int(10),
            Value::Int(2),
            Value::Int(10),
            Value::Int(1),
            Value::Int(5),
        ])
    );
    assert_eq!(m.live_bytes(), before);

    // 初期束縛に別型を渡しても既存関数 f の参照先（大域 x = 1）は変わらない
    let res2 = m
        .eval_with("(tuple x (f))", &[("x", "text", Value::text("oops"))])
        .unwrap();
    assert_eq!(res2, Value::tuple(vec![Value::text("oops"), Value::Int(2)]));
    assert_eq!(m.live_bytes(), before);

    // 初期束縛の重複キーは後勝ち、組み込み名・大域名の遮蔽も許可される
    let res3 = m
        .eval_with(
            "(tuple x map)",
            &[
                ("x", "i64", Value::Int(100)),
                ("x", "i64", Value::Int(200)),
                ("map", "i64", Value::Int(42)),
            ],
        )
        .unwrap();
    assert_eq!(res3, Value::tuple(vec![Value::Int(200), Value::Int(42)]));

    // 局所変数・仮引数による大域名・組み込み名の遮蔽も引き続き許可される
    assert_eq!(
        m.eval("(let (x 99 map 7) (+ x map ((fn (x) x) 3)))")
            .unwrap(),
        Value::Int(109)
    );

    // 未定義名ヒントに初期束縛名が含まれる
    let err = m
        .eval_with("bonuz", &[("bonus", "i64", Value::Int(5))])
        .unwrap_err();
    assert_eq!(err.rule(), "expr.name");
    assert!(
        err.message.contains("did you mean 'bonus'?"),
        "got: {}",
        err.message
    );
    assert_eq!(m.live_bytes(), before);

    // ゼロヒープ API (eval_with_vals) でも同じレキシカル挙動
    let v10 = m.val_int(10);
    let out = m
        .eval_with_vals(
            "(tuple x (f) (invoke (fn () x)) ((make)))",
            &[("x", "i64", v10)],
        )
        .unwrap();
    assert_eq!(out.to_string(), "(tuple 10 2 10 1)");
    m.release_val(v10);
    // eval_with_vals の戻り値は last_result に保持されるため、次の eval_ref で解放される。
    assert_eq!(m.eval_ref("0").unwrap().as_int(), Some(0));
    assert_eq!(m.live_bytes(), before);
}

#[test]
fn duplicate_definitions_builtins_and_module_collisions_are_rejected() {
    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);

    // def / defn / bind の重複定義
    b.eval("(def a 1) (defn f () -> i64 2)").unwrap();
    let e = b.eval("(def a 10)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.def");
    assert_eq!((e.at.line, e.at.col), (1, 1));

    let e = b.eval("(defn f () -> i64 20)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.defn");
    assert_eq!((e.at.line, e.at.col), (1, 1));

    let e = b.bind("a", "i64", &Value::Int(30)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.def");
    assert_eq!((e.at.line, e.at.col), (0, 0));

    // 組み込み名の上書き禁止（def / defn / bind / load_module）
    let e = b.eval("(def map 1)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.def");

    let e = b.eval("(defn + ((x i64)) -> i64 x)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.defn");

    let e = b.bind("pi", "i64", &Value::Int(3)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.def");
    assert_eq!((e.at.line, e.at.col), (0, 0));

    let e = b.load_module("map", "(def x 1)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.module");
    assert_eq!((e.at.line, e.at.col), (0, 0));

    // 既存大域名と同じ名前の load_module は拒否
    let e = b.load_module("a", "(def x 1)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.module");
    assert_eq!((e.at.line, e.at.col), (0, 0));

    // 読み込み済みモジュール名と同じ名前の def / defn / bind は import の有無によらず拒否
    b.load_module("game", "(def hp 100) (export hp)").unwrap();
    let e = b.eval("(def game 1)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.def");

    let e = b.eval("(defn game () -> i64 1)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.defn");

    let e = b.bind("game", "i64", &Value::Int(1)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.def");
    assert_eq!((e.at.line, e.at.col), (0, 0));

    // 成功済みモジュールの再読込は拒否
    let e = b.load_module("game", "(def hp 200)").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Runtime);
    assert_eq!(e.rule(), "expr.module");
    assert_eq!((e.at.line, e.at.col), (0, 0));

    // 同一スコープでの同一モジュールの再 import は許可
    b.eval("(import game) (import game)").unwrap();
    assert_eq!(b.eval("game.hp").unwrap(), Value::Int(100));

    // モジュール内の重複定義・組み込み上書き・値↔import 衝突は拒否
    let e = b.load_module("dup_m", "(def x 1) (def x 2)").unwrap_err();
    assert_eq!(e.rule(), "expr.def");
    let e = b
        .load_module("bi_m", "(defn map ((x i64)) -> i64 x)")
        .unwrap_err();
    assert_eq!(e.rule(), "expr.defn");
    let e = b
        .load_module("imp_c1", "(def game 1) (import game)")
        .unwrap_err();
    assert_eq!(e.rule(), "expr.module");
    let e = b
        .load_module("imp_c2", "(import game) (def game 1)")
        .unwrap_err();
    assert_eq!(e.rule(), "expr.def");

    // 読み込みに失敗したモジュール名は再試行できる
    b.load_module("dup_m", "(def x 1) (export x)").unwrap();
    // 別モジュール同士で同じ定義名（hp / x）を持つことは許可
    b.load_module(
        "mod_a",
        "(def val 10) (import game) (import game) (export val)",
    )
    .unwrap();
    b.load_module("mod_b", "(def val 20) (export val)").unwrap();
    let mut m = b.freeze().unwrap();
    assert_eq!(
        m.eval("(+ mod_a.val mod_b.val dup_m.x)").unwrap(),
        Value::Int(31)
    );
}

#[test]
fn failed_builder_eval_rolls_back_all_added_bindings_without_alloc() {
    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);
    b.load_module("lib", "(def k 9) (export k)").unwrap();
    b.eval("(def keep 7)").unwrap();
    let baseline = b.live_bytes();

    // ゼロ除算で失敗した評価の def / defn / import はすべて取り消される
    let err = b
        .eval("(import lib) (def a [1 2 3]) (defn g () -> (list i64) a) (/ 1 0)")
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(b.live_bytes(), baseline);
    assert_eq!(b.eval("keep").unwrap(), Value::Int(7));
    assert_eq!(b.eval("a").unwrap_err().rule(), "expr.name");
    assert_eq!(b.eval("g").unwrap_err().rule(), "expr.name");
    assert_eq!(b.eval("lib.k").unwrap_err().rule(), "expr.name");

    // 取り消された名前は同じ Builder で再利用できる
    b.eval("(import lib) (def a 2) (defn g () -> i64 (+ a lib.k))")
        .unwrap();
    assert_eq!(b.eval("(g)").unwrap(), Value::Int(11));

    // 重複名エラーでも途中の新規定義は取り消される
    let before_dup = b.live_bytes();
    let err = b.eval("(def temp \"hello\") (def a 99)").unwrap_err();
    assert_eq!(err.rule(), "expr.def");
    assert_eq!(b.live_bytes(), before_dup);
    assert_eq!(b.eval("temp").unwrap_err().rule(), "expr.name");
    b.eval("(def temp \"ok\")").unwrap();
    assert_eq!(b.eval("temp").unwrap(), Value::text("ok"));

    // ステップ上限超過でも取り消される
    let mut alloc2 = HeapAllocator;
    let mut b2 = Builder::new(
        Limits {
            steps: 3,
            ..Limits::default()
        },
        &mut alloc2,
    );
    let err = b2.eval("(def s 1) (+ 1 2 3 4)").unwrap_err();
    assert_eq!(err.kind, ErrorKind::TooManySteps);
    assert_eq!(b2.live_bytes(), 0);
    b2.eval("(def s 42)").unwrap();
    assert_eq!(b2.eval("s").unwrap(), Value::Int(42));

    // メモリ上限超過（固定アリーナ）でも追加確保なしで安全に取り消される
    let mut buf = vec![0u8; 64 * 1024];
    let mut arena = ArenaAllocator::new(&mut buf);
    let mut b3 = Builder::new(
        Limits {
            memory_bytes: 16 * 1024,
            ..Limits::default()
        },
        &mut arena,
    );
    b3.eval("(def ok_val 1)").unwrap();
    let live_before = b3.live_bytes();
    let err = b3
        .eval("(def rolled_back [1 2 3]) (range 0 10000)")
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::TooMuchMemory);
    assert_eq!(b3.live_bytes(), live_before);
    assert_eq!(b3.eval("ok_val").unwrap(), Value::Int(1));
    assert_eq!(b3.eval("rolled_back").unwrap_err().rule(), "expr.name");
    b3.eval("(def rolled_back 5)").unwrap();
    assert_eq!(b3.eval("rolled_back").unwrap(), Value::Int(5));
}

#[test]
fn eval_with_many_bindings_last_wins_hints_and_no_leak() {
    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);
    b.eval("(defn sq ((n i64)) -> i64 (* n n))").unwrap();
    let mut m = b.freeze().unwrap();

    // 9 個超（deferred_initial 経路）で重複後勝ち・クロージャ捕捉・未使用オブジェクト解放を確認
    let mut owned_names: Vec<String> = (0..50).map(|i| format!("item_{i}")).collect();
    owned_names.push("item_0".to_string()); // 重複（後ろ勝ち）
    owned_names.push("damage".to_string());
    let mut bindings: Vec<(&str, &str, Value)> = Vec::new();
    for (i, name) in owned_names.iter().enumerate() {
        if i == 50 {
            bindings.push((name.as_str(), "i64", Value::Int(7)));
        } else if i == 51 {
            bindings.push((name.as_str(), "text", Value::text("unused_heap_str")));
        } else if i % 5 == 0 {
            bindings.push((name.as_str(), "text", Value::text(format!("str_{i}"))));
        } else {
            bindings.push((name.as_str(), "i64", Value::Int(i as i64)));
        }
    }

    let live_before = m.live_bytes();
    let val = m
        .eval_with("((fn () (+ (sq item_0) item_49)))", &bindings)
        .unwrap();
    assert_eq!(val, Value::Int(7 * 7 + 49));
    assert_eq!(m.live_bytes(), live_before);

    // 未参照の初期束縛も未定義名の候補ヒントに出る
    let err = m.eval_with("damge", &bindings).unwrap_err();
    assert_eq!(err.rule(), "expr.name");
    assert!(
        err.message.contains("did you mean 'damage'?"),
        "unexpected message: {}",
        err.message
    );
    assert_eq!(m.live_bytes(), live_before);
}
