use crate::{Builder, ErrorKind, HeapAllocator, Limits, Span, StackFrame, Value, eval, eval_with};

#[test]
fn runtime_and_limit_errors_record_call_backtrace() {
    let src = "\
(defn inner (x) (/ x 0))
(defn outer (x) (+ 1 (inner x)))
(outer 5)";
    let err = eval(src).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Runtime);
    assert_eq!((err.at.line, err.at.col), (1, 17));
    assert_eq!(
        err.backtrace(),
        &[
            StackFrame {
                module: None,
                name: Some("inner".to_string()),
                caller_module: None,
                at: Span::new(2, 22),
                tail_calls: 0,
                tail_target_module: None,
                tail_target_name: None,
                repeat_count: 1,
                from_host: false,
                via_builtin: None,
            },
            StackFrame {
                module: None,
                name: Some("outer".to_string()),
                caller_module: None,
                at: Span::new(3, 1),
                tail_calls: 0,
                tail_target_module: None,
                tail_target_name: None,
                repeat_count: 1,
                from_host: false,
                via_builtin: None,
            },
        ]
    );
    let rendered = err.to_string();
    assert!(rendered.contains("1:17: division by zero"));
    assert!(rendered.contains("in inner (at 2:22)"));
    assert!(rendered.contains("in outer (at 3:1)"));

    // 1. 末尾呼び出し後も入口の関数名・位置を残し、省略数と遷移先を記録する。
    let tail_src = "\
(defn g (x) (/ x 0))
(defn f (x) (g x))
(f 1)";
    let tail_err = eval(tail_src).unwrap_err();
    assert_eq!(tail_err.backtrace().len(), 1);
    assert_eq!(tail_err.backtrace()[0].name.as_deref(), Some("f"));
    assert_eq!(tail_err.backtrace()[0].at, Span::new(3, 1));
    assert_eq!(tail_err.backtrace()[0].tail_calls, 1);
    assert!(
        tail_err
            .to_string()
            .contains("in g (tail-called from f at 3:1) [tail calls elided: 1]")
    );

    // 2 & 3. モジュール名、モジュールをまたぐ末尾呼び出し、ホスト入口フレーム。
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(Limits::default(), &mut alloc);
    m.load_module("ma", "(defn f (x) (/ x 0))").unwrap();
    m.load_module(
        "mb",
        "(import ma)\n(defn f (x) (+ 1 (ma.f x)))\n(defn g (x) (+ 1 (f x)))",
    )
    .unwrap();
    let mod_err = m.call_in("mb", "g", &[Value::Int(1)]).unwrap_err();
    assert_eq!(mod_err.module(), Some("ma"));
    assert!(mod_err.to_string().starts_with("ma 1:13: division by zero"));
    let mod_bt = mod_err.format_backtrace();
    assert!(mod_bt.contains("in ma.f (at mb 2:18)"));
    assert!(mod_bt.contains("in mb.f (at mb 3:18)"));
    assert!(mod_bt.contains("in mb.g (called from host)"));
    assert!(!mod_bt.contains("0:0"));

    // モジュール間の末尾呼び出し: mc.f が ma.f を末尾呼び出しする。
    m.load_module(
        "mc",
        "(import ma)\n(defn f (x) (ma.f x))\n(defn g (x) (+ 1 (f x)))",
    )
    .unwrap();
    let tail_mod_err = m.call_in("mc", "g", &[Value::Int(1)]).unwrap_err();
    assert_eq!(tail_mod_err.module(), Some("ma"));
    let tail_mod_rendered = tail_mod_err.to_string();
    assert!(tail_mod_rendered.starts_with("ma 1:13: division by zero"));
    assert!(
        tail_mod_rendered
            .contains("in ma.f (tail-called from mc.f at mc 3:18) [tail calls elided: 1]")
    );
    assert!(tail_mod_rendered.contains("in mc.g (called from host)"));

    // モジュールのトップレベル評価で失敗した場合、最外フレームに caller_module を記録する。
    let top_mod_err = m
        .load_module("mc2", "(import ma)\n(def z (+ 1 (ma.f 1)))")
        .unwrap_err();
    assert_eq!(top_mod_err.module(), Some("ma"));
    assert_eq!(
        top_mod_err.backtrace()[0].caller_module.as_deref(),
        Some("mc2")
    );
    assert!(top_mod_err.to_string().contains("in ma.f (at mc2 2:13)"));

    // モジュール関数から呼ばれたトップレベル由来ラムダは、見出しに外側のモジュール名を付けない。
    m.load_module("hof", "(defn apply_it (f) (+ 1 (f 1)))")
        .unwrap();
    let lambda_err = m
        .eval("(import hof)\n(hof.apply_it (fn (x) (/ x 0)))")
        .unwrap_err();
    assert_eq!(lambda_err.module(), None);
    assert!(lambda_err.to_string().starts_with("2:23: division by zero"));
    assert!(lambda_err.to_string().contains("in hof.apply_it (at 2:1)"));

    // load_module の字句・構文エラーにもモジュール名を付与する。
    let lex_err = m
        .load_module("bad_lex", "(defn f () \"unterminated")
        .unwrap_err();
    assert_eq!(lex_err.kind, ErrorKind::Lex);
    assert_eq!(lex_err.module(), Some("bad_lex"));
    assert!(lex_err.to_string().starts_with("bad_lex 1:"));

    let parse_err = m
        .load_module("bad_parse", "(defn f (x)\n  (+ x")
        .unwrap_err();
    assert_eq!(parse_err.kind, ErrorKind::Parse);
    assert_eq!(parse_err.module(), Some("bad_parse"));
    assert!(parse_err.to_string().starts_with("bad_parse 2:3:"));

    // 4. TooDeep での連続同一フレーム圧縮と、深い相互再帰での中間フレーム省略。
    let deep_limits = Limits {
        steps: 100_000,
        depth: 66,
        memory_bytes: 16 * 1024 * 1024,
    };
    let deep_err = eval_with("(defn d (n) (+ 1 (d n)))\n(d 0)", &[], deep_limits).unwrap_err();
    assert_eq!(deep_err.kind, ErrorKind::TooDeep);
    assert_eq!(deep_err.backtrace().len(), 2);
    assert_eq!(deep_err.backtrace()[0].repeat_count, 65);
    assert!(deep_err.to_string().contains("in d (at 1:18) ×65"));

    let mutual_src = "\
(defn a (n) (+ 1 (b n)))
(defn b (n) (+ 1 (a n)))
(a 0)";
    let mutual_err = eval_with(mutual_src, &[], deep_limits).unwrap_err();
    assert_eq!(mutual_err.kind, ErrorKind::TooDeep);
    assert_eq!(mutual_err.backtrace().len(), 64);
    assert!(mutual_err.omitted_frames() > 0);
    assert!(mutual_err.to_string().contains("frames omitted"));

    // 5. 組み込みの高階関数（fold 等）からの呼び出し元の記録。
    let fold_err = eval("(fold (fn (a x) (/ a x)) 10 [1 0])").unwrap_err();
    assert_eq!(fold_err.backtrace().len(), 1);
    assert_eq!(fold_err.backtrace()[0].via_builtin.as_deref(), Some("fold"));
    assert!(
        fold_err
            .to_string()
            .contains("in <lambda> (called by fold at 1:1)")
    );

    // 6. C FFI の rin_last_error と早期リターン時の履歴クリア。
    unsafe {
        let cm = crate::ffi::rin_new(
            crate::ffi::rin_limits_default(),
            crate::ffi::RinAllocator {
                ctx: std::ptr::null_mut(),
                alloc: None,
                dealloc: None,
            },
        );
        let c_src = std::ffi::CString::new(src).unwrap();
        let res = crate::ffi::rin_build(cm, c_src.as_ptr());
        assert!(!res.ok);
        let bt_ptr = (*crate::ffi::rin_last_error(cm)).backtrace;
        assert!(!bt_ptr.is_null());
        let bt_str = std::ffi::CStr::from_ptr(bt_ptr).to_str().unwrap();
        assert!(bt_str.contains("in inner (at 2:22)"));
        assert!(bt_str.contains("in outer (at 3:1)"));

        // 早期リターンによるエラーでも前回の呼び出し履歴は消去する。
        let bad_utf8 = [0xffu8, 0x00];
        let bad_res = crate::ffi::rin_build(cm, bad_utf8.as_ptr().cast());
        assert!(!bad_res.ok);
        assert!((*crate::ffi::rin_last_error(cm)).backtrace.is_null());

        crate::ffi::rin_free(cm);
    }
}
