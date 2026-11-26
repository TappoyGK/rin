use rin::{Value, eval};

#[test]
fn line_comments_end_at_newline_or_eof() {
    for newline in ["\n", "\r\n"] {
        let source = format!(
            ";;; 定義の説明{newline}(def x; 空白なし{newline}6){newline};; 行全体{newline}(/ x; 除算{newline}2); 末尾のコメント"
        );
        assert_eq!(eval(&source).unwrap(), Value::Int(3));
    }
}

#[test]
fn comment_markers_inside_literals_are_preserved() {
    assert_eq!(eval(r#""; // ;; ;;;""#).unwrap(), Value::str("; // ;; ;;;"));
    assert_eq!(eval("\\;").unwrap(), Value::Char(';'));
}

#[test]
fn comments_break_postfix_attachment() {
    assert_eq!(eval("[10][0]").unwrap(), Value::Int(10));
    assert_eq!(eval("[10]; comment\n[0]").unwrap(), eval("[0]").unwrap());
}

#[test]
fn slash_sequences_are_identifiers_not_comments() {
    assert_eq!(eval("(def // 7) //").unwrap(), Value::Int(7));
    assert_eq!(eval("(/ 6 2)").unwrap(), Value::Int(3));
}

#[test]
fn diagnostics_keep_positions_after_comments() {
    let error = eval("; 日本語\r\n;; comment\r\n  missing; end").unwrap_err();
    assert_eq!((error.at.line, error.at.col), (3, 3));
}
