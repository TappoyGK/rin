//! VM の既定深さを 2 MiB のネイティブスタック上で検証する。abort は子プロセスに隔離する。
use rin::{Builder, ErrorKind, HeapAllocator, Limits};

fn chain(count: usize, builtin: &str, leaf: &str) -> String {
    let mut source = String::new();
    let (params, args, result) = match builtin {
        "fold" => ("acc x", "0 1", "acc"),
        "any" | "all" | "find" | "filter" => ("x", "1", "true"),
        "flat_map" => ("x", "1", "[x]"),
        _ => ("x", "1", "x"),
    };
    for i in 0..count {
        let body = if i + 1 == count {
            if leaf == "ok" {
                result.to_string()
            } else {
                "(/ 1 0)".to_string()
            }
        } else if builtin == "direct" {
            format!("(+ 1 (f{} x))", i + 1)
        } else if builtin == "fold" {
            format!("(fold f{} acc [0 x]) acc", i + 1)
        } else {
            format!("({builtin} f{} [0 x]) {result}", i + 1)
        };
        let body = if builtin == "direct" {
            body
        } else {
            let first = if matches!(builtin, "any" | "find") {
                "false"
            } else {
                result
            };
            format!("(if (== x 0) {first} (let () {body}))")
        };
        source.push_str(&format!("(defn f{i} ({params}) {body})\n"));
    }
    source.push_str(&format!("(f0 {args})"));
    source
}

#[test]
fn default_depth_is_safe_on_two_mib_stack() {
    const CHILD: &str = "RIN_VM_STACK_CHILD";
    if let Ok(case) = std::env::var(CHILD) {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let parts: Vec<_> = case.split(':').collect();
                let count = parts[1].parse::<usize>().unwrap();
                let source = chain(count, parts[0], parts[2]);
                let mut alloc = HeapAllocator;
                let mut machine = Builder::new(Limits::default(), &mut alloc);
                let (definitions, call) = source.rsplit_once('\n').unwrap();
                machine.eval(definitions).unwrap();
                let mut machine = machine.freeze().unwrap();
                let live_before = machine.live_bytes();
                let result = if parts[3] == "host" {
                    let args = if parts[0] == "fold" {
                        vec![rin::Value::Int(0), rin::Value::Int(1)]
                    } else {
                        vec![rin::Value::Int(1)]
                    };
                    machine.call("f0", &args)
                } else {
                    machine.eval(call)
                };
                if count > 200 {
                    let error = result.unwrap_err();
                    assert_eq!(error.kind, ErrorKind::TooDeep);
                    assert_eq!(error.rule(), "eval.limits");
                } else if parts[2] == "error" {
                    let error = result.unwrap_err();
                    assert_eq!(error.kind, ErrorKind::Runtime);
                    assert_eq!(error.rule(), "op.arith");
                    assert_eq!(error.at.line, count);
                    assert_eq!(error.backtrace().len(), count.min(64));
                    assert_eq!(error.omitted_frames(), count.saturating_sub(64));
                    assert_eq!(
                        error.backtrace().last().unwrap().from_host,
                        parts[3] == "host"
                    );
                    assert_eq!(
                        error.backtrace()[0].name.as_deref(),
                        Some(format!("f{}", count - 1).as_str())
                    );
                    assert_eq!(
                        error.backtrace()[0].via_builtin.as_deref(),
                        (parts[0] != "direct").then_some(parts[0])
                    );
                } else {
                    let expected = match parts[0] {
                        "direct" => rin::Value::Int(count as i64),
                        "fold" => rin::Value::Int(0),
                        "find" | "any" | "all" | "filter" => rin::Value::Bool(true),
                        "flat_map" => rin::eval("[1]").unwrap(),
                        _ => rin::Value::Int(1),
                    };
                    assert_eq!(result.unwrap(), expected);
                }
                assert_eq!(machine.live_bytes(), live_before, "temporary values leaked");
                // エラー後にも同じ Machine を使えること。
                assert_eq!(machine.eval("(+ 1 2)").unwrap(), rin::Value::Int(3));
            })
            .unwrap()
            .join()
            .unwrap();
        return;
    }
    for builtin in [
        "direct", "map", "filter", "flat_map", "fold", "find", "any", "all", "sort_by",
    ] {
        for (count, leaf) in [
            (80, "error"),
            (199, "ok"),
            (200, "ok"),
            (200, "error"),
            (201, "ok"),
        ] {
            for entry in ["eval", "host"] {
                let case = format!("{builtin}:{count}:{leaf}:{entry}");
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "default_depth_is_safe_on_two_mib_stack",
                        "--nocapture",
                    ])
                    .env(CHILD, &case)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{case}: {}\n{}\n{}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}

#[test]
fn callback_arity_errors_release_owned_arguments() {
    for builtin in [
        "map", "filter", "flat_map", "fold", "find", "any", "all", "sort_by",
    ] {
        let arity = if builtin == "fold" { 3 } else { 2 };
        for argc in 0..=5 {
            if argc == arity {
                continue;
            }
            let mut alloc = HeapAllocator;
            let mut machine = Builder::new(Limits::default(), &mut alloc);
            let live_before = machine.live_bytes();
            let args = " [\"owned\"]".repeat(argc);
            let error = machine.eval(&format!("({builtin}{args})")).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Runtime, "{builtin}/{argc}");
            assert_eq!(error.rule(), "fn.list", "{builtin}/{argc}");
            assert_eq!(machine.live_bytes(), live_before, "{builtin}/{argc}");
            assert_eq!(machine.eval("(+ 1 2)").unwrap(), rin::Value::Int(3));
        }
    }
}
