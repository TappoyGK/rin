//! Issue #61: `text` 改名、浮動小数点表示、テンプレート `"${式}"`、`len`/添字/`slice`/比較高速化の網羅テスト。
#![cfg(feature = "std-value")]

use rin::{Builder, ErrorKind, HeapAllocator, Limits, Schema, Value, ValueTag, eval};

#[test]
fn float_display_matches_suz_format() {
    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);

    for (val, expected) in [
        (0.0_f64, "0.0"),
        (-0.0_f64, "-0.0"),
        (1.0_f64, "1.0"),
        (-1.0_f64, "-1.0"),
        (0.1_f64, "0.1"),
        (100.0_f64, "100.0"),
        (1.5e-8_f64, "1.5e-8"),
        (1e-7_f64, "1.0e-7"),
        (1e-6_f64, "0.000001"),
        (1.5e-6_f64, "0.0000015"),
        (1e20_f64, "100000000000000000000.0"),
        (1e21_f64, "1.0e21"),
        (-1e21_f64, "-1.0e21"),
        (f64::NAN, "nan"),
        (f64::INFINITY, "inf"),
        (f64::NEG_INFINITY, "-inf"),
        (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
        (f64::MAX, "1.7976931348623157e308"),
    ] {
        let v = Value::Float(val);
        assert_eq!(v.to_string(), expected, "Value Display {val:?}");

        let bv = b.val_float(val);
        let vr = b.value_ref(bv);
        assert_eq!(vr.to_string(), expected, "ValueRef Display {val:?}");
        b.release_val(bv);

        if val.is_finite() || val == f64::INFINITY {
            let round = eval(&v.to_string()).unwrap();
            let Value::Float(r) = round else {
                panic!("expected float for {expected}");
            };
            assert_eq!(r.to_bits(), val.to_bits(), "roundtrip {expected}");
        }
    }
}

#[test]
fn template_interpolation_escapes_and_errors() {
    // 基本・複数埋め込み・型ごとの表示
    assert_eq!(
        eval(
            "(let (n 42 x 1.0 b true c \\あ t \"hi\" id 'foo) \"${n},${x},${b},${c},${t},${id}\")"
        )
        .unwrap(),
        Value::text("42,1.0,true,あ,hi,foo")
    );
    assert_eq!(
        eval("\"${1.5e-8} ${-0.0} ${nan} ${inf} ${(- inf)}\"").unwrap(),
        Value::text("1.5e-8 -0.0 nan inf -inf")
    );

    // 入れ子テンプレート・波括弧を含む構造体リテラル・左から右の評価順
    assert_eq!(
        eval("(let (x 3) \"outer=${\"inner=${(+ x 1)}\"}\")").unwrap(),
        Value::text("outer=inner=4")
    );
    assert_eq!(
        eval("\"p=${Point { x: 10 y: 20 }.x}\"").unwrap(),
        Value::text("p=10")
    );

    // `$` のエスケープと単独 `$`
    assert_eq!(
        eval("\"\\${x} \\$ $a $100 $\"").unwrap(),
        Value::text("${x} $ $a $100 $")
    );

    // 値の表示で `$` が `\$` になり、読み直すと元に戻る
    for raw in ["${x}", "$a", "$", "a$b${c}"] {
        let v = Value::text(raw);
        let displayed = v.to_string();
        assert!(!displayed.contains("${") || displayed.contains("\\${"));
        assert_eq!(
            eval(&displayed).unwrap(),
            v,
            "roundtrip {raw:?} via {displayed}"
        );
    }

    // 埋め込みのない文字列は match パターンで使える
    assert_eq!(
        eval("(match \"ok\" (\"ng\" 1) (\"ok\" 2) (_ 3))").unwrap(),
        Value::Int(2)
    );

    // 生の改行は字句エラー（評価・rin fmt 双方）
    for bad_nl in ["\"a\nb\"", "\"a\rb\"", "\"${1}\nb\""] {
        let err = eval(bad_nl).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Lex, "{bad_nl:?}: {err}");
        assert_eq!(err.rule(), "src.string", "{bad_nl:?}: {err}");
        let ferr = rin::fmt::format(bad_nl).unwrap_err();
        assert_eq!(ferr.kind, ErrorKind::Lex, "fmt {bad_nl:?}: {ferr}");
    }

    // 埋め込めない型は expr.template の実行時エラー
    for bad_val in [
        "\"${[1 2]}\"",
        "\"${none}\"",
        "\"${some(1)}\"",
        "\"${Point { x: 1 }}\"",
        "\"${(tuple 1 2)}\"",
    ] {
        let err = eval(bad_val).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Runtime, "{bad_val}: {err}");
        assert_eq!(err.rule(), "expr.template", "{bad_val}: {err}");
    }

    // 埋め込み内の式エラー位置が正確
    let err = eval("\"prefix ${(/ 1 0)}\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.at.line, 1);
    assert_eq!(err.at.col, 11);

    // 埋め込めない型のエラー位置はその埋め込み式自体を指す
    let err = eval("(def xs [1]) \"aaa ${1} ${xs}\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.rule(), "expr.template");
    assert_eq!(err.at.line, 1);
    assert_eq!(err.at.col, 26);

    // 空の `${}` はわかりやすい構文エラー
    let err = eval("\"a${}b\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert!(
        err.message.contains("empty template interpolation"),
        "got: {}",
        err.message
    );

    // quote / quasiquote 内のテンプレート文字列は構文エラー
    for quoted in ["'\"a${1}\"", "`\"a${1}\""] {
        let err = eval(quoted).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse, "{quoted}: {err}");
        assert!(
            err.message.contains("template interpolation"),
            "{quoted}: {}",
            err.message
        );
    }

    // deftest のテスト名が text でないときのエラーに text が含まれる
    let report = rin::test::run(
        &[("test.rin", "(deftest 123 (check true))")],
        &[],
        Limits::default(),
    );
    assert_eq!(report.errors.len(), 1);
    let err = &report.errors[0].error;
    assert_eq!(err.kind, ErrorKind::Parse);
    assert!(err.message.contains("text"), "got: {}", err.message);

    // テンプレート入れ子上限超過のメッセージは深さによらず統一される
    for depth in [256usize, 257, 300] {
        let deep = format!("{}1{}", "\"${".repeat(depth), "}\"".repeat(depth));
        let err = eval(&deep).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse, "depth={depth}: {err}");
        assert_eq!(err.rule(), "src.nesting-limit", "depth={depth}: {err}");
        assert_eq!(
            err.message, "syntax nesting limit exceeded (256)",
            "depth={depth}: {err}"
        );
        let ferr = rin::fmt::format(&deep).unwrap_err();
        assert_eq!(
            ferr.rule(),
            "src.nesting-limit",
            "fmt depth={depth}: {ferr}"
        );
        assert_eq!(
            ferr.message, "syntax nesting limit exceeded (256)",
            "fmt depth={depth}: {ferr}"
        );
    }

    // フォーマッタはテンプレート全体を原文のまま保つ
    let src = "\"a=${(+ 1   2)} b=${Point { x: 1 }.x}\"";
    assert_eq!(rin::fmt::format(src).unwrap().trim_end(), src);
}

#[test]
fn text_type_name_len_index_slice_compare_and_sharing() {
    // 型名が text（Value / Schema / エラーメッセージ）
    assert_eq!(Value::text("hi").type_name(), "text");
    let schema = Schema::parse("text").unwrap();
    assert!(schema.check(&Value::text("hi")).is_ok());
    assert!(schema.check(&Value::Int(1)).is_err());
    assert!(Schema::parse("string").is_err());

    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);
    let tv = b.val_text("abc").unwrap();
    assert_eq!(b.value_ref(tv).tag(), ValueTag::Text);
    assert_eq!(b.value_ref(tv).as_text(), Some("abc"));
    b.release_val(tv);

    // ASCII / 非 ASCII の len・添字・get・slice
    assert_eq!(eval("(len \"\")").unwrap(), Value::Int(0));
    assert_eq!(eval("(len \"abc\")").unwrap(), Value::Int(3));
    assert_eq!(eval("(len \"あいう\")").unwrap(), Value::Int(3));
    assert_eq!(eval("(len \"a😀b\")").unwrap(), Value::Int(3));
    assert_eq!(eval("(len \"a\\0b\")").unwrap(), Value::Int(3));

    assert_eq!(eval("\"abc\"[0]").unwrap(), Value::Char('a'));
    assert_eq!(eval("\"abc\"[2]").unwrap(), Value::Char('c'));
    assert_eq!(eval("\"あいう\"[1]").unwrap(), Value::Char('い'));
    assert_eq!(eval("\"a😀b\"[1]").unwrap(), Value::Char('😀'));
    assert_eq!(
        eval("(get \"abc\" 1)").unwrap(),
        Value::some(Value::Char('b'))
    );
    assert_eq!(eval("(get \"abc\" 3)").unwrap(), Value::None);
    assert_eq!(eval("(get \"abc\" -1)").unwrap(), Value::None);
    assert_eq!(
        eval("(get \"あいう\" 2)").unwrap(),
        Value::some(Value::Char('う'))
    );
    assert_eq!(eval("(get \"あいう\" 3)").unwrap(), Value::None);

    let err = eval("\"abc\"[3]").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.rule(), "expr.index");

    assert_eq!(eval("(slice \"abcdef\" 1 4)").unwrap(), Value::text("bcd"));
    assert_eq!(
        eval("(slice \"あいうえお\" 1 4)").unwrap(),
        Value::text("いうえ")
    );
    assert_eq!(eval("(slice \"あいう\" 2 2)").unwrap(), Value::text(""));
    let err = eval("(slice \"あいう\" 2 4)").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.rule(), "fn.list");

    // 比較（== / != / < / <= / > / >=）
    assert_eq!(eval("(== \"abc\" \"abc\")").unwrap(), Value::Bool(true));
    assert_eq!(eval("(!= \"abc\" \"abcd\")").unwrap(), Value::Bool(true));
    assert_eq!(eval("(< \"apple\" \"banana\")").unwrap(), Value::Bool(true));
    assert_eq!(eval("(<= \"a\" \"a\")").unwrap(), Value::Bool(true));
    assert_eq!(eval("(> \"い\" \"あ\")").unwrap(), Value::Bool(true));
    assert_eq!(eval("(>= \"b\" \"a\")").unwrap(), Value::Bool(true));
    assert!(eval("(< \"a\" 'a)").is_err());

    // 同じ文字列リテラルの比較は同一実体として追加ステップなしで完了する
    let long = "x".repeat(1000);
    let mut alloc2 = HeapAllocator;
    let mut m = Builder::new(
        Limits {
            steps: 10,
            ..Limits::default()
        },
        &mut alloc2,
    );
    m.eval(&format!("(defn same () (== \"{long}\" \"{long}\"))"))
        .unwrap();
    let mut m = m.freeze().unwrap();
    assert_eq!(m.call("same", &[]).unwrap(), Value::Bool(true));

    // 長さが違う文字列の比較も内容を見ずに少ないステップで終わる
    let long2 = "x".repeat(1001);
    let mut alloc3 = HeapAllocator;
    let mut m3 = Builder::new(
        Limits {
            steps: 10,
            ..Limits::default()
        },
        &mut alloc3,
    );
    m3.eval(&format!("(defn diff () (== \"{long}\" \"{long2}\"))"))
        .unwrap();
    let mut m3 = m3.freeze().unwrap();
    assert_eq!(m3.call("diff", &[]).unwrap(), Value::Bool(false));
}
