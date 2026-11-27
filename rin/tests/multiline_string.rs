//! Issue #66: 複数行文字列リテラル (`"""..."""`) の網羅テスト。
#![cfg(feature = "std-value")]

use rin::{Builder, ErrorKind, HeapAllocator, Limits, Value, eval};

#[test]
fn multiline_basic_indentation_and_boundaries() {
    // 基本のインデント除去と相対インデント保持
    let src = "\"\"\"\n  hello\n    world\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("hello\n  world"));

    // 閉じ """ を左にずらして全行に先頭空白を残す
    let src = "\"\"\"\n    a\n    b\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("  a\n  b"));

    // タブインデントの一致
    let src = "\"\"\"\n\t\ta\n\t\t\tb\n\t\t\"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("a\n\tb"));

    // 空文字列・末尾改行の境界（LF 1 / 2 / 3 個）
    assert_eq!(eval("\"\"\"\n\"\"\"").unwrap(), Value::text(""));
    assert_eq!(eval("\"\"\"\n\n\"\"\"").unwrap(), Value::text(""));
    assert_eq!(eval("\"\"\"\n\n\n\"\"\"").unwrap(), Value::text("\n"));
    assert_eq!(
        eval("\"\"\"\n  hello\n\n  \"\"\"").unwrap(),
        Value::text("hello\n")
    );

    // 空白のみの行は基準より浅くても深くても空行になる
    let src = "\"\"\"\n    a\n \n      \n\t\n    b\n    \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("a\n\n\n\nb"));

    // エスケープで生成した空白や改行はインデント判定に使わない
    let src = "\"\"\"\n  \\u{20}\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text(" "));
    let err = eval("\"\"\"\n\\u{20}\n  \"\"\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Lex);
    assert_eq!(err.rule(), "src.string");

    // 閉じ """ の直後に通常コードを置ける
    let src = "(def s \"\"\"\n  hi\n  \"\")\ns";
    // 上は `""` なので閉じ `"""` が正しいケースを検証
    assert!(eval(src).is_err());
    let src = "(def s \"\"\"\n  hi\n  \"\"\")\ns";
    assert_eq!(eval(src).unwrap(), Value::text("hi"));

    // CRLF は LF に正規化される
    let src = "\"\"\"\r\n  a\r\n  b\r\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("a\nb"));
}

#[test]
fn multiline_quotes_escapes_and_interpolation() {
    // 単独の " と "" はエスケープ不要、""" はエスケープして書ける
    let src = "\"\"\"\n  \"a\" \"\"b\"\" \\\"\\\"\\\" \"\"\\\"\n  \"\"\"";
    assert_eq!(
        eval(src).unwrap(),
        Value::text("\"a\" \"\"b\"\" \"\"\" \"\"\"")
    );

    // 共通エスケープと $ / \$
    let src = "\"\"\"\n  \\0\\n\\r\\t\\\\\\u{3042}\\${x}$a\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("\0\n\r\t\\あ${x}$a"));

    // 埋め込みと複数行の式（外側のインデント検査対象外）
    let src = "\"\"\"\n  a${(+ 1\n2)}b\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("a3b"));

    // 埋め込み結果にはインデント除去・CRLF 正規化を適用しない
    let src = "\"\"\"\n  ${\"x\\n  y\"}\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("x\n  y"));
    let src = "\"\"\"\n  ${\"x\\r\\ny\"}\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("x\r\ny"));

    // 埋め込みが空文字列になってもその行は非空行として検査される
    let src = "\"\"\"\n  ${\"\"}\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text(""));
    let err = eval("\"\"\"\n${\"\"}\n  \"\"\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Lex);
    assert_eq!(err.rule(), "src.string");

    // 埋め込み内に複数行文字列を入れ子にでき、文字リテラル \" \{ \} \; も安全
    let src = "\"\"\"\n  outer:${\"\"\"\n    inner:${\\\"}\n    \"\"\"}\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("outer:inner:\""));

    let src = "\"\"\"\n  braces:${(let (a \\{ b \\}) \"${a}${b}\")}\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("braces:{}"));

    // 埋め込み内のコメントに } や """ があっても壊れない
    let src = "\"\"\"\n  v=${(+ 1 ; } \"\"\"\n    2)}\n  \"\"\"";
    assert_eq!(eval(src).unwrap(), Value::text("v=3"));

    // 埋め込みのない複数行文字列は match パターン・quote / quasiquote で使える
    let src = "(match \"a\\nb\" (\"\"\"\n  a\n  b\n  \"\"\" 1) (_ 0))";
    assert_eq!(eval(src).unwrap(), Value::Int(1));
    assert_eq!(eval("'\"\"\"\n  hi\n  \"\"\"").unwrap(), Value::text("hi"));
    assert_eq!(eval("`\"\"\"\n  hi\n  \"\"\"").unwrap(), Value::text("hi"));

    // 埋め込みのある複数行文字列は quote / quasiquote で構文エラー
    let err = eval("'\"\"\"\n  ${1}\n  \"\"\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);

    // 同一の複数行リテラルは通常リテラルと永続領域で共有される
    let long = "x".repeat(1000);
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(
        Limits {
            steps: 10,
            ..Limits::default()
        },
        &mut alloc,
    );
    m.eval(&format!(
        "(defn same () (== \"{long}\" \"\"\"\n  {long}\n  \"\"\"))"
    ))
    .unwrap();
    let mut m = m.freeze().unwrap();
    assert_eq!(m.call("same", &[]).unwrap(), Value::Bool(true));
}

#[test]
fn multiline_errors_and_spans() {
    // 1. 浅い非空行
    let err = eval("\"\"\"\n  ok\n shallow\n  \"\"\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Lex);
    assert_eq!(err.rule(), "src.string");
    assert_eq!(err.at.line, 3);
    assert_eq!(err.at.col, 1);

    // タブとスペースの不一致
    let err = eval("\"\"\"\n\ta\n  \"\"\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Lex);
    assert_eq!(err.rule(), "src.string");

    // 2. 開始 """ の直後の空白・コメント・文字
    for bad in [
        "\"\"\" \n  a\n  \"\"\"",
        "\"\"\" ; comment\n  a\n  \"\"\"",
        "\"\"\"abc\"\"\"",
    ] {
        let err = eval(bad).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Lex, "{bad:?}: {err}");
        assert_eq!(err.rule(), "src.string", "{bad:?}: {err}");
    }

    // 3. 単独の CR
    let err = eval("\"\"\"\n  a\r  b\n  \"\"\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Lex);
    assert_eq!(err.rule(), "src.string");

    // 4. 配置に反する未エスケープ """
    let err = eval("\"\"\"\n  hello\"\"\"\n  \"\"\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Lex);
    assert_eq!(err.rule(), "src.string");

    // 行末のバックスラッシュによる行継続は未知のエスケープ
    let err = eval("\"\"\"\n  a\\\n  b\n  \"\"\"").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Lex);
    assert_eq!(err.rule(), "src.escape");

    // 未閉鎖の複数行文字列
    let err = eval("\"\"\"\n  hello\n").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Lex);
    assert_eq!(err.rule(), "src.string");

    // 埋め込み内の実行時エラー位置は元のソース位置を指す
    let src = "\"\"\"\n  line1\n  val=${(/ 1 0)}\n  \"\"\"";
    let err = eval(src).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.at.line, 3);
    assert_eq!(err.at.col, 9);

    // 埋め込めない型のエラー位置は元のソースの式を指す
    let src = "(def xs [1])\n\"\"\"\n  ${xs}\n  \"\"\"";
    let err = eval(src).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!(err.rule(), "expr.template");
    assert_eq!(err.at.line, 3);
    assert_eq!(err.at.col, 5);

    // 入れ子上限（256 段）超過は通常テンプレートと同じメッセージ
    let deep = format!("{}1{}", "\"\"\"\n${".repeat(257), "}\n\"\"\"".repeat(257));
    let err = eval(&deep).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Parse);
    assert_eq!(err.rule(), "src.nesting-limit");
    assert_eq!(err.message, "syntax nesting limit exceeded (256)");
}

#[test]
fn multiline_formatter_preserves_content_and_closing_indent() {
    // 周囲の字下げが変わっても、開始 """ から閉じ """ までの内容と閉じ行の字下げは変えない
    let src = "(def   s\n    \"\"\"\n      line1\n        line2 ${(+ 1  2)}\n      \"\"\")\n";
    let formatted = rin::fmt::format(src).unwrap();
    assert!(
        formatted.contains("\"\"\"\n      line1\n        line2 ${(+ 1  2)}\n      \"\"\""),
        "formatted was:\n{formatted}"
    );
    // 冪等性
    assert_eq!(rin::fmt::format(&formatted).unwrap(), formatted);
    // 整形前後で評価結果が一致する
    let orig = format!("{src}\ns");
    let fmt_eval = format!("{formatted}\ns");
    assert_eq!(eval(&orig).unwrap(), eval(&fmt_eval).unwrap());
}
