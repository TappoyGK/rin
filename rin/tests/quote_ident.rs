//! Issue #71: 字句整理、`quote` / `quasiquote`、`ident` 型、`(tuple ...)`、Display 往復の網羅テスト。
#![cfg(feature = "std-value")]

use std::ffi::CString;

use rin::ffi::{
    RinAllocator, RinErrorKind, RinTag, rin_as_ident, rin_drop, rin_eval, rin_free, rin_freeze,
    rin_ident, rin_last_error, rin_limits_default, rin_new, rin_tag,
};
use rin::{Builder, ErrorKind, HeapAllocator, Limits, Schema, Value, ValueTag, eval};

#[test]
fn lex_char_vs_quote_and_unicode_escapes() {
    assert_eq!(eval("'a'").unwrap(), Value::Char('a'));
    assert_eq!(eval("'\\n'").unwrap(), Value::Char('\n'));
    assert_eq!(eval("'\\r'").unwrap(), Value::Char('\r'));
    assert_eq!(eval("'\\t'").unwrap(), Value::Char('\t'));
    assert_eq!(eval("'\\\\'").unwrap(), Value::Char('\\'));
    assert_eq!(eval("'\\''").unwrap(), Value::Char('\''));
    assert_eq!(eval("'\\\"'").unwrap(), Value::Char('"'));
    assert_eq!(eval("'\\0'").unwrap(), Value::Char('\0'));
    assert_eq!(eval("'\\u{20}'").unwrap(), Value::Char(' '));
    assert_eq!(eval("'\\u{3042}'").unwrap(), Value::Char('あ'));

    // `'X'` の直後に区切り以外の文字が続く場合、エスケープでない X が前置記号・開き括弧のときだけ quote から読み直す
    assert_eq!(
        eval("'('a 'b)").unwrap(),
        eval("[['quote 'a] ['quote 'b]]").unwrap()
    );
    assert_eq!(eval("'('a')").unwrap(), eval("['a']").unwrap());
    assert_eq!(
        eval("'['a 'b]").unwrap(),
        eval("['list ['quote 'a] ['quote 'b]]").unwrap()
    );
    assert_eq!(
        eval("(def x 1) ``(a ',',x)").unwrap(),
        eval("['quasiquote ['a ['quote ['unquote ['quote 1]]]]]").unwrap()
    );
    let err = eval("(def x 1) `(a ',',x)").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "src.syntax");

    // 単独の開き括弧・前置記号の文字リテラルは直後が区切りなら引き続き文字として読まれる
    assert_eq!(
        eval("(list '(' '[' '{' ',' '`')").unwrap(),
        Value::list(vec![
            Value::Char('('),
            Value::Char('['),
            Value::Char('{'),
            Value::Char(','),
            Value::Char('`'),
        ])
    );

    // それ以外の `'X'`（英字・数字・`;`・`"`・非ASCII・エスケープ等）の直後に区切り以外が続く場合は src.char 字句エラー
    for bad in [
        "'a'b",
        "(list 'a'(list))",
        "(list 'a'[1])",
        "(list 'a'\"s\")",
        "(list 'a''b')",
        "(list ';'a\n1)",
        "(list '\"'x \" \"y\")",
        "(list '1'a 1)",
        "(list '9'(+ 1 2))",
        "(list 'é'x 1)",
        "'\\u{28}'a",
        "'\\n'a",
    ] {
        let err = eval(bad).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Lex, "{bad}: {err}");
        assert_eq!(err.rule(), "src.char", "{bad}: {err}");
        let fmt_err = rin::fmt::format(bad).unwrap_err();
        assert_eq!(fmt_err.kind, ErrorKind::Lex, "fmt {bad}: {fmt_err}");
        assert_eq!(fmt_err.rule(), "src.char", "fmt {bad}: {fmt_err}");
    }

    // 非 ASCII 文字の字句エラー・未知エスケープエラーで文字化けしない（評価・rin fmt 双方）
    for (src, rule, expected) in [
        ("é", "src.token", "'é'"),
        ("'é", "src.token", "'é'"),
        ("あ", "src.token", "'あ'"),
        ("\"\\é\"", "src.escape", "\\é"),
        ("'\\é'", "src.escape", "\\é"),
    ] {
        let err = eval(src).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Lex, "{src}: {err}");
        assert_eq!(err.rule(), rule, "{src}: {err}");
        assert!(
            err.message.contains(expected),
            "{src}: expected {expected:?} in {:?}",
            err.message
        );
        let fmt_err = rin::fmt::format(src).unwrap_err();
        assert_eq!(fmt_err.kind, ErrorKind::Lex, "fmt {src}: {fmt_err}");
        assert_eq!(fmt_err.rule(), rule, "fmt {src}: {fmt_err}");
        assert!(
            fmt_err.message.contains(expected),
            "fmt {src}: expected {expected:?} in {:?}",
            fmt_err.message
        );
    }

    // 識別子直後の `'` は字句エラー
    for bad_ident in ["(def y' 1)", "foo'bar", "x'"] {
        let err = eval(bad_ident).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Lex, "{bad_ident}: {err}");
        assert_eq!(err.rule(), "src.ident", "{bad_ident}: {err}");
    }

    // quote としての `'`
    assert_eq!(eval("'foo").unwrap(), Value::ident("foo"));
    assert_eq!(eval("''a'").unwrap(), Value::Char('a'));
    assert_eq!(eval("'123").unwrap(), Value::Int(123));
    assert_eq!(eval("'\"hi\"").unwrap(), Value::str("hi"));
    assert_eq!(
        eval("'(a b)").unwrap(),
        Value::list(vec![Value::ident("a"), Value::ident("b")])
    );

    // quasiquote 外（quote の中も含む）の `,` / `,@` / `(unquote ...)` / `(unquote_splicing ...)` は構文エラー
    for bad in [
        ",x",
        ",@xs",
        "(unquote x)",
        "(unquote_splicing xs)",
        "'(a ,x)",
        "'(a ,@x)",
        "'(unquote x)",
        "'(unquote_splicing x)",
    ] {
        let err = eval(bad).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse, "{bad}: {err}");
        assert_eq!(err.rule(), "src.syntax", "{bad}: {err}");
    }
}

#[test]
fn quote_literals_sugar_normalization_and_canonical_eval() {
    // スカラー・キーワード
    assert_eq!(eval("'foo").unwrap(), Value::ident("foo"));
    assert_eq!(eval("'def").unwrap(), Value::ident("def"));
    assert_eq!(eval("'+").unwrap(), Value::ident("+"));
    assert_eq!(eval("'1").unwrap(), Value::Int(1));
    assert_eq!(eval("'1.5").unwrap(), Value::Float(1.5));
    assert_eq!(eval("'inf").unwrap(), Value::Float(f64::INFINITY));
    assert!(matches!(eval("'nan").unwrap(), Value::Float(x) if x.is_nan()));
    assert_eq!(eval("'true").unwrap(), Value::Bool(true));
    assert_eq!(eval("'false").unwrap(), Value::Bool(false));
    assert_eq!(eval("'none").unwrap(), Value::None);
    assert_eq!(eval("'()").unwrap(), Value::empty_list());
    assert_eq!(eval("(quote (a b))").unwrap(), eval("'(a b)").unwrap());

    // 糖衣構文の正規 S 式への正規化
    for (sugar, canonical) in [
        ("'[a b]", "'(list a b)"),
        ("'[]", "'(list)"),
        ("'a.b", "'(field a b)"),
        ("'a.b.c", "'(field (field a b) c)"),
        ("'xs[i]", "'(index xs i)"),
        ("'some(x)", "'(some x)"),
        ("'ok(x)", "'(ok x)"),
        ("'err(x)", "'(err x)"),
        ("'Type.Variant", "'(variant Type Variant)"),
        ("'Type.Variant(x)", "'(variant Type Variant x)"),
        ("'Point { x: 1 y: 2 }", "'(struct Point (x 1) (y 2))"),
        ("'Point {}", "'(struct Point)"),
        ("'(with p { x: 1 y: 2 })", "'(with p (x 1) (y 2))"),
        ("'(with p {})", "'(with p)"),
    ] {
        assert_eq!(
            eval(sugar).unwrap(),
            eval(canonical).unwrap(),
            "{sugar} vs {canonical}"
        );
    }

    // 正規 S 式形式は通常のソース構文としても評価できる
    for (sugar, canonical) in [
        ("[1 2 3]", "(list 1 2 3)"),
        ("Point { x: 10 y: 20 }", "(struct Point (x 10) (y 20))"),
        ("Point {}", "(struct Point)"),
        ("Point { x: 10 }.x", "(field (struct Point (x 10)) x)"),
        ("[10 20 30][1]", "(index (list 10 20 30) 1)"),
        ("some(42)", "(some 42)"),
        ("ok(1)", "(ok 1)"),
        ("err(\"ng\")", "(err \"ng\")"),
        ("Color.Red", "(variant Color Red)"),
        ("Color.Rgb(255)", "(variant Color Rgb 255)"),
        (
            "(with Point { x: 1 y: 2 } { x: 9 })",
            "(with (struct Point (x 1) (y 2)) (x 9))",
        ),
        ("(with Point { x: 1 } {})", "(with (struct Point (x 1)))"),
    ] {
        assert_eq!(
            eval(sugar).unwrap(),
            eval(canonical).unwrap(),
            "eval {sugar} vs {canonical}"
        );
    }
}

#[test]
fn quasiquote_unquote_splicing_and_nesting() {
    assert_eq!(
        eval("(let (x 2 xs [3 4]) `(1 ,x ,@xs 5))").unwrap(),
        eval("[1 2 3 4 5]").unwrap()
    );
    assert_eq!(
        eval("(let (xs [1 2]) `[0 ,@xs 3])").unwrap(),
        eval("['list 0 1 2 3]").unwrap()
    );
    assert_eq!(
        eval("(let (xs []) `(a ,@xs b))").unwrap(),
        eval("['a 'b]").unwrap()
    );
    assert_eq!(
        eval("(let (v 7) `(with p { x: ,v }))").unwrap(),
        eval("['with 'p ['x 7]]").unwrap()
    );
    assert_eq!(
        eval("(let (x 10) (quasiquote (a (unquote x) (unquote_splicing [20 30]))))").unwrap(),
        eval("['a 10 20 30]").unwrap()
    );

    // 入れ子 quasiquote
    assert_eq!(
        eval("``(a ,(+ 1 2) ,,(+ 1 2))").unwrap(),
        eval("['quasiquote ['a ['unquote ['+ 1 2]] ['unquote 3]]]").unwrap()
    );

    // quasiquote の中の quote は深さを変えない
    assert_eq!(
        eval("(let (x 1) `(a '(b ,x)))").unwrap(),
        eval("['a ['quote ['b 1]]]").unwrap()
    );
    // quote の中の quasiquote 内の unquote / unquote_splicing は評価せずデータとして残す
    assert_eq!(
        eval("'(a `(b ,x))").unwrap(),
        eval("['a ['quasiquote ['b ['unquote 'x]]]]").unwrap()
    );
    assert_eq!(
        eval("(quote (quasiquote (a (unquote b))))").unwrap(),
        eval("['quasiquote ['a ['unquote 'b]]]").unwrap()
    );
    assert_eq!(
        eval("'(a `(b ,@xs))").unwrap(),
        eval("['a ['quasiquote ['b ['unquote_splicing 'xs]]]]").unwrap()
    );
    // 前置記号の `'` と `` ` `` を括弧なしで続けても quote 内は評価されない
    assert_eq!(
        eval("(def x 1) '`'`,x").unwrap(),
        eval("['quasiquote ['quote ['quasiquote ['unquote 'x]]]]").unwrap()
    );
    assert_eq!(
        eval("(defn boom () (/ 1 0)) '`'`,(boom)").unwrap(),
        eval("['quasiquote ['quote ['quasiquote ['unquote ['boom]]]]]").unwrap()
    );
    assert_eq!(
        eval("(def x 1) '`'(a `(b ,x))").unwrap(),
        eval("['quasiquote ['quote ['a ['quasiquote ['b ['unquote 'x]]]]]]").unwrap()
    );
    // 単独の `` '`' `` は引き続きバッククォート文字リテラル
    assert_eq!(eval("'`'").unwrap(), Value::Char('`'));
    assert_eq!(eval("(== '`' '\\u{60}')").unwrap(), Value::Bool(true));
    // quote の中の quasiquote でも深さ 0 を超えて unquote を重ねると構文エラー
    let err = eval("'(a `(b ,,x))").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "src.syntax");
    // 評価される `,expr` の中（深さ 0）に `,` を書くと構文エラー
    let err = eval("(let (y 1) `(a ,(list ,y)))").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "src.syntax");
    // 単一オペランド位置での `,@`（深さ 2 の `,,@xs` を含む）は構文エラー
    let err = eval("(let (xs [1 2]) `(a `(b ,,@xs)))").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "src.syntax");

    // リスト以外を `,@` で展開すると実行時エラー
    let err = eval("`(1 ,@42 2)").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.rule(), "expr.quasiquote");

    // リスト要素位置以外での `,@` は構文エラー
    for bad in ["`,@xs", "`some(,@xs)", "`Point { x: ,@xs }"] {
        let err = eval(bad).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse, "{bad}: {err}");
    }
}

#[test]
fn ident_operations_conversions_match_schema_and_ffi() {
    // 比較・順序・ソート
    assert_eq!(eval("(== 'foo 'foo)").unwrap(), Value::Bool(true));
    assert_eq!(eval("(!= 'foo 'bar)").unwrap(), Value::Bool(true));
    assert_eq!(eval("(< 'apple 'banana)").unwrap(), Value::Bool(true));
    assert_eq!(eval("(<= 'a 'a)").unwrap(), Value::Bool(true));
    assert_eq!(eval("(> 'z 'a)").unwrap(), Value::Bool(true));
    assert_eq!(eval("(>= 'b 'a)").unwrap(), Value::Bool(true));
    assert_eq!(
        eval("(sort ['c 'a 'b])").unwrap(),
        eval("['a 'b 'c]").unwrap()
    );

    // string との比較は実行時エラー
    for bad in ["(== 'foo \"foo\")", "(!= 'foo \"foo\")", "(< 'foo \"foo\")"] {
        let err = eval(bad).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Runtime, "{bad}: {err}");
        assert_eq!(err.rule(), "op.cmp", "{bad}: {err}");
    }

    // ident->text / text->ident
    assert_eq!(eval("(ident->text 'hello)").unwrap(), Value::str("hello"));
    assert_eq!(
        eval("(text->ident \"hello\")").unwrap(),
        Value::some(Value::ident("hello"))
    );
    assert_eq!(eval("(text->ident \"\")").unwrap(), Value::None);
    assert_eq!(eval("(text->ident \"123\")").unwrap(), Value::None);
    assert_eq!(eval("(text->ident \"a'b\")").unwrap(), Value::None);
    assert_eq!(eval("(text->ident \"a b\")").unwrap(), Value::None);

    let err = eval("(ident->text \"hello\")").unwrap_err();
    assert_eq!(err.rule(), "fn.convert");
    let err = eval("(text->ident 'hello)").unwrap_err();
    assert_eq!(err.rule(), "fn.convert");

    // match パターン
    let m = eval("(match 'bar ('foo 1) ('bar 2) (_ 3))").unwrap();
    assert_eq!(m, Value::Int(2));

    // Schema DSL
    let schema = Schema::parse("ident").unwrap();
    assert!(schema.check(&Value::ident("foo")).is_ok());
    assert!(schema.check(&Value::str("foo")).is_err());

    // Rust zero-heap API & Value import validation
    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);
    let iv = b.val_ident("alpha").unwrap();
    assert_eq!(b.value_ref(iv).tag(), ValueTag::Ident);
    assert_eq!(b.value_ref(iv).as_ident(), Some("alpha"));
    assert!(b.val_ident("123").is_err());
    b.release_val(iv);

    let err = b.bind("bad_id", &Value::ident("1 bad'")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.rule(), "host.value");
    // 深い経路（depth >= 32）でも Value::Ident を検査する
    let mut deep = Value::ident("1 bad'");
    for _ in 0..35 {
        deep = Value::some(deep);
    }
    let err = b.bind("bad_deep", &deep).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.rule(), "host.value");

    // C FFI
    unsafe {
        let m = rin_new(
            rin_limits_default(),
            RinAllocator {
                ctx: std::ptr::null_mut(),
                alloc: None,
                dealloc: None,
            },
        );
        let name = CString::new("foo_bar").unwrap();
        let v = rin_ident(m, name.as_ptr());
        assert!(!v.is_null());
        assert_eq!(rin_tag(m, v), RinTag::Ident);
        let mut len = 0usize;
        let ptr = rin_as_ident(m, v, &mut len);
        assert!(!ptr.is_null());
        assert_eq!(std::slice::from_raw_parts(ptr, len), b"foo_bar");
        rin_drop(m, v);

        // 無効な識別子は NULL + RIN_ERR_HOST
        let bad = CString::new("123bad").unwrap();
        let bv = rin_ident(m, bad.as_ptr());
        assert!(bv.is_null());
        let err = rin_last_error(m);
        assert!(!err.is_null());
        assert_eq!((*err).kind, RinErrorKind::Host);

        assert!(rin_freeze(m));
        let src = CString::new("'hello").unwrap();
        let res = rin_eval(m, src.as_ptr());
        assert!(res.ok);
        assert_eq!(rin_tag(m, res.value), RinTag::Ident);
        let ptr = rin_as_ident(m, res.value, &mut len);
        assert_eq!(std::slice::from_raw_parts(ptr, len), b"hello");
        rin_free(m);
    }
}

#[test]
fn tuple_syntax_and_display_roundtrip() {
    assert_eq!(
        eval("(tuple 1 2)").unwrap(),
        Value::tuple(vec![Value::Int(1), Value::Int(2)])
    );
    assert_eq!(
        eval("(tuple 1 \"a\" true)").unwrap(),
        Value::tuple(vec![Value::Int(1), Value::str("a"), Value::Bool(true)])
    );

    // 0 要素・1 要素・旧構文は構文エラー
    for bad in ["(tuple)", "(tuple 1)", "(1, 2)", "(1,)"] {
        let err = eval(bad).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse, "{bad}: {err}");
    }

    // Display 往復（Value と ValueRef の両方）
    let mut alloc = HeapAllocator;
    let mut b = Builder::new(Limits::default(), &mut alloc);
    for (src, expected_display) in [
        ("[1 2 3]", "[1 2 3]"),
        ("P { x: 1 y: 2 }", "P { x: 1 y: 2 }"),
        ("P {}", "P {}"),
        ("(tuple 1 2)", "(tuple 1 2)"),
        ("'foo", "'foo"),
    ] {
        let v = eval(src).unwrap();
        assert_eq!(v.to_string(), expected_display);
        assert_eq!(eval(&v.to_string()).unwrap(), v);

        let vr = b.eval_ref(src).unwrap();
        assert_eq!(vr.to_string(), expected_display);
    }
}

#[test]
fn quote_respects_nesting_limit() {
    let ok = format!("'{}1{}", "(".repeat(254), ")".repeat(254));
    assert!(eval(&ok).is_ok());

    let too_deep = format!("'{}1{}", "(".repeat(256), ")".repeat(256));
    let err = eval(&too_deep).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "src.nesting-limit");
}
