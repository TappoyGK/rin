//! 深い構文入れ子入力に対して小さいスタック（256 KiB）で安全に構文エラーを返すことを子プロセス隔離で検証する。

use rin::{ErrorKind, Schema, eval};

const STACK_BYTES: usize = 256 * 1024;
const LIMIT: usize = 256;

fn nested_add(depth: usize) -> String {
    if depth <= 1 {
        return "1".to_string();
    }
    let mut s = "(+ 1 ".repeat(depth - 1);
    s.push('1');
    s.push_str(&")".repeat(depth - 1));
    s
}

fn nested_list(depth: usize) -> String {
    if depth <= 1 {
        return "1".to_string();
    }
    let mut s = "[".repeat(depth - 1);
    s.push('1');
    s.push_str(&"]".repeat(depth - 1));
    s
}

fn nested_let(depth: usize) -> String {
    if depth <= 1 {
        return "1".to_string();
    }
    let mut s = "(let (x 1) ".repeat(depth - 1);
    s.push('x');
    s.push_str(&")".repeat(depth - 1));
    s
}

fn nested_fn(depth: usize) -> String {
    if depth <= 1 {
        return "0".to_string();
    }
    let mut s = "(fn () ".repeat(depth - 1);
    s.push('0');
    s.push_str(&")".repeat(depth - 1));
    s
}

fn nested_field(depth: usize) -> String {
    let mut s = "x".to_string();
    s.push_str(&".a".repeat(depth.saturating_sub(1)));
    s
}

fn nested_index(depth: usize) -> String {
    let mut s = "x".to_string();
    s.push_str(&"[0]".repeat(depth.saturating_sub(1)));
    s
}

fn nested_schema(depth: usize) -> String {
    if depth <= 1 {
        return "i64".to_string();
    }
    let mut s = "(option ".repeat(depth - 1);
    s.push_str("i64");
    s.push_str(&")".repeat(depth - 1));
    s
}

fn nested_schema_variant(depth: usize) -> String {
    if depth <= 1 {
        return "i64".to_string();
    }
    let mut s = "(variant V (C ".repeat(depth - 1);
    s.push_str("i64");
    s.push_str(&"))".repeat(depth - 1));
    s
}

fn nested_schema_struct(depth: usize) -> String {
    if depth <= 1 {
        return "i64".to_string();
    }
    let mut s = "(struct S (f ".repeat(depth - 1);
    s.push_str("i64");
    s.push_str(&"))".repeat(depth - 1));
    s
}

fn nested_schema_tuple(depth: usize) -> String {
    if depth <= 1 {
        return "i64".to_string();
    }
    let mut s = "(tuple i64 ".repeat(depth - 1);
    s.push_str("i64");
    s.push_str(&")".repeat(depth - 1));
    s
}

fn run_in_small_stack() {
    let handle = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(|| {
            // 上限直前と上限ちょうどが構文解析・コンパイル・評価まで成功すること。
            for d in [LIMIT - 1, LIMIT] {
                assert!(eval(&nested_add(d)).is_ok(), "add depth {d} should succeed");
                // `len` で包むことで、#31 範囲の `Value::export` 再帰を避けつつ構文・コンパイル・評価・解放の深さを検証する。
                let wrapped_list = format!("(len {})", nested_list(d - 1));
                assert!(eval(&wrapped_list).is_ok(), "list depth {d} should succeed");
                assert!(eval(&nested_let(d)).is_ok(), "let depth {d} should succeed");
                assert!(eval(&nested_fn(d)).is_ok(), "fn depth {d} should succeed");
                assert!(
                    Schema::parse(&nested_schema(d)).is_ok(),
                    "schema depth {d} should succeed"
                );
            }

            // 上限ちょうどのスキーマ（variant / struct / tuple）で Clone と Debug も溢れないこと。
            for s_src in [
                nested_schema_variant(LIMIT),
                nested_schema_struct(LIMIT),
                nested_schema_tuple(LIMIT),
            ] {
                let s = Schema::parse(&s_src).expect("deep schema should parse");
                let cloned = s.clone();
                let dbg = format!("{cloned:?}");
                assert!(!dbg.is_empty());
            }

            // 後置アクセスと括弧の混在（上限ちょうどは成功、超過は構文エラー）。
            let mix_ok = format!("(+ 1 {})", nested_field(LIMIT - 1));
            // `x` が未定義なので実行時エラーにはなるが構文解析・コンパイルは成功する。
            let mix_ok_err = eval(&mix_ok).unwrap_err();
            assert_eq!(mix_ok_err.kind, ErrorKind::Runtime);

            let mix_over = format!("(+ 1 {})", nested_field(LIMIT));
            assert_eq!(eval(&mix_over).unwrap_err().kind, ErrorKind::Parse);

            // 上限超過（257 と 10,000）が abort せず Parse エラーになること。
            for d in [LIMIT + 1, 10_000] {
                assert_eq!(eval(&nested_add(d)).unwrap_err().kind, ErrorKind::Parse);
                assert_eq!(eval(&nested_list(d)).unwrap_err().kind, ErrorKind::Parse);
                assert_eq!(eval(&nested_let(d)).unwrap_err().kind, ErrorKind::Parse);
                assert_eq!(eval(&nested_fn(d)).unwrap_err().kind, ErrorKind::Parse);
                assert_eq!(eval(&nested_field(d)).unwrap_err().kind, ErrorKind::Parse);
                assert_eq!(eval(&nested_index(d)).unwrap_err().kind, ErrorKind::Parse);
                assert_eq!(
                    Schema::parse(&nested_schema(d)).unwrap_err().kind,
                    ErrorKind::Parse
                );
            }

            // 閉じ括弧が足りない深い入力でもエラー時の解放で溢れないこと。
            let unclosed_add = "(+ 1 ".repeat(10_000);
            assert_eq!(eval(&unclosed_add).unwrap_err().kind, ErrorKind::Parse);
            let unclosed_list = "[".repeat(10_000);
            assert_eq!(eval(&unclosed_list).unwrap_err().kind, ErrorKind::Parse);
        })
        .expect("spawn small stack thread");

    handle
        .join()
        .expect("small stack thread panicked or aborted");
}

#[test]
fn syntax_nesting_limit_is_safe_on_small_stack_in_isolated_process() {
    if std::env::var_os("RIN_NESTING_CHILD").is_some() {
        run_in_small_stack();
        return;
    }

    let exe = std::env::current_exe().expect("current_exe");
    let status = std::process::Command::new(exe)
        .arg("--exact")
        .arg("syntax_nesting_limit_is_safe_on_small_stack_in_isolated_process")
        .arg("--nocapture")
        .env("RIN_NESTING_CHILD", "1")
        .status()
        .expect("spawn child test process");

    assert!(
        status.success(),
        "isolated child process failed or aborted with status: {status}"
    );
}
