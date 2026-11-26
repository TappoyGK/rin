//! 1 バイト刻みの OOM 網羅テスト (`oomsweep`) と 256 KiB スタック上での深さ 100,000 非末尾再帰テスト。

use rin::{ArenaAllocator, Builder, ErrorKind, HeapAllocator, Limits};

#[test]
fn oom_sweep_every_byte_across_phases() {
    let programs = [
        "(let (xs (range 0 20)) (fold + 0 (map (fn (x) (* x 2)) xs)))",
        "Card { id: \"ember\", cost: 3, tags: [\"fire\" \"fast\"] }",
        "(sort_by (fn (x) (- 0 x)) [3 1 4 1 5 9 2 6])",
    ];

    // 1. 読み込み・コンパイル・評価の 1 バイト刻み OOM 網羅
    for src in programs {
        let mut succeeded_at = None;
        let mut limit = 0usize;
        while limit <= 65_536 {
            let mut alloc = HeapAllocator;
            let mut builder = Builder::new(
                Limits {
                    memory_bytes: limit,
                    ..Limits::default()
                },
                &mut alloc,
            );
            match builder.eval_ref(src) {
                Ok(_) => {
                    succeeded_at = Some(limit);
                    break;
                }
                Err(e) => {
                    assert_eq!(
                        e.kind(),
                        ErrorKind::TooMuchMemory,
                        "limit={limit} src={src} err={e}"
                    );
                    assert_eq!(builder.live_bytes(), 0, "leak at limit={limit} src={src}");
                }
            }
            limit += if limit < 4096 { 1 } else { 64 };
        }
        assert!(succeeded_at.is_some(), "program never succeeded: {src}");
    }

    // 2. モジュール読み込みの OOM 網羅
    {
        let mod_src = "(export f) (defn f (x) (map (fn (n) (+ n x)) [1 2 3 4]))";
        let mut succeeded = false;
        let mut limit = 0usize;
        while limit <= 65_536 {
            let mut alloc = HeapAllocator;
            let mut builder = Builder::new(
                Limits {
                    memory_bytes: limit,
                    ..Limits::default()
                },
                &mut alloc,
            );
            match builder.load_module_ref("m", mod_src) {
                Ok(()) => {
                    succeeded = true;
                    break;
                }
                Err(e) => {
                    assert_eq!(e.kind(), ErrorKind::TooMuchMemory, "limit={limit} err={e}");
                    assert_eq!(builder.live_bytes(), 0, "module leak at limit={limit}");
                }
            }
            limit += if limit < 4096 { 1 } else { 64 };
        }
        assert!(succeeded, "load_module_ref never succeeded");
    }

    // 3. 凍結後の評価・呼び出しの OOM 網羅（失敗後も live_bytes が基準値へ戻ること）
    {
        let mut backing = vec![0u8; 256 * 1024];
        let mut arena = ArenaAllocator::new(&mut backing);
        let mut builder = Builder::new(
            Limits {
                memory_bytes: 32 * 1024,
                ..Limits::default()
            },
            &mut arena,
        );
        builder
            .eval_ref("(defn make_list (n) (map (fn (x) (+ x 1)) (range 0 n)))")
            .unwrap();
        let mut machine = builder.freeze().unwrap();
        let baseline = machine.live_bytes();

        // 十分小さい n は成功し、大きすぎる n は TooMuchMemory でリークしない。
        let arg_ok = machine.val_int(10);
        assert!(machine.call_ref("make_list", &[arg_ok]).is_ok());
        machine.release_val(arg_ok);

        let arg_big = machine.val_int(5000);
        let err = machine.call_ref("make_list", &[arg_big]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TooMuchMemory);
        machine.release_val(arg_big);
        assert_eq!(machine.live_bytes(), baseline);
    }
}

#[test]
fn deep_non_tail_recursion_100k_on_256kb_stack() {
    let handle = std::thread::Builder::new()
        .name("rin-256kb-stack".into())
        .stack_size(256 * 1024)
        .spawn(|| {
            let mut alloc = HeapAllocator;
            let mut builder = Builder::new(
                Limits {
                    steps: 5_000_000,
                    depth: 100_010,
                    memory_bytes: 128 * 1024 * 1024,
                },
                &mut alloc,
            );
            let res = builder
                .eval_ref(
                    r#"
                    (defn count_down (n)
                      (if (== n 0)
                        0
                        (+ 1 (count_down (- n 1)))))
                    (count_down 100000)
                    "#,
                )
                .unwrap();
            assert_eq!(res.as_int(), Some(100_000));
        })
        .expect("spawn 256kb stack thread");
    handle.join().expect("256kb stack thread panicked");
}
