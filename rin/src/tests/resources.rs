use crate::{
    ArenaAllocator, Builder, ErrorKind, HeapAllocator, Limits, Value, eval_with,
    eval_with_allocator,
};

#[test]
fn recursive_defn_and_step_limit() {
    let src = r#"
        (defn loop_forever (n) (loop_forever (+ n 1)))
        (loop_forever 0)
    "#;
    let err = eval_with(
        src,
        &[],
        Limits {
            steps: 50,
            depth: 200,
            memory_bytes: 1024 * 1024,
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::TooManySteps);
}

#[test]
fn step_limit_error_reports_exact_span() {
    let limits = |steps| Limits {
        steps,
        depth: 200,
        memory_bytes: 1024 * 1024,
    };
    let e1 = eval_with("(let (x 1) [x (/ 1 0)])", &[], limits(1)).unwrap_err();
    assert_eq!(e1.kind, ErrorKind::TooManySteps);
    assert_eq!((e1.at.line, e1.at.col), (1, 9));

    let e2 = eval_with("(+ 1\n   zzz)", &[], limits(2)).unwrap_err();
    assert_eq!(e2.kind, ErrorKind::TooManySteps);
    assert_eq!((e2.at.line, e2.at.col), (1, 4));

    let e3 = eval_with("(def a 1) (def a (+ a 1)) a", &[], limits(1)).unwrap_err();
    assert_eq!(e3.kind, ErrorKind::TooManySteps);
    assert_eq!((e3.at.line, e3.at.col), (1, 8));
}

#[test]
fn arena_allocator_reuses_slots_across_many_calls() {
    let mut backing = vec![0u8; 64 * 1024];
    let mut arena = ArenaAllocator::new(&mut backing);
    {
        let mut machine = Builder::new(Limits::default(), &mut arena);
        machine
            .eval("(defn sum_double (n) (fold + 0 (map (fn (x) (* x 2)) (range 0 n))))")
            .unwrap();
        let baseline = machine.live_bytes();

        for _ in 0..500 {
            let res = machine.eval("(sum_double 50)").unwrap();
            assert_eq!(res, Value::Int(2450));
            assert_eq!(machine.live_bytes(), baseline);
        }
    }
    assert!(arena.used() > 0 && arena.used() <= 64 * 1024);
}

#[test]
fn arena_exhaustion_returns_too_much_memory() {
    let mut tiny = vec![0u8; 256];
    let mut arena = ArenaAllocator::new(&mut tiny);
    let err = eval_with_allocator("(range 0 20)", &[], Limits::default(), &mut arena).unwrap_err();
    assert_eq!(err.kind, ErrorKind::TooMuchMemory);
}

#[test]
fn error_paths_do_not_leak_pool_slots() {
    let mut backing = vec![0u8; 64 * 1024];
    let mut arena = ArenaAllocator::new(&mut backing);
    let mut machine = Builder::new(
        Limits {
            steps: 200,
            depth: 50,
            memory_bytes: 64 * 1024,
        },
        &mut arena,
    );
    machine
        .eval(
            r#"
            (defn boom (n)
              (let (tmp (range 0 10))
                (if (== n 0) (/ tmp[0] 0) (boom (- n 1)))))
            (defn loop_step (n)
              (let (tmp [1 2 3])
                (loop_step (+ n (len tmp)))))
            "#,
        )
        .unwrap();
    let baseline = machine.live_bytes();

    for _ in 0..20 {
        let err = machine.eval("(boom 3)").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Runtime);
        assert_eq!(machine.live_bytes(), baseline);

        let step_err = machine.eval("(loop_step 0)").unwrap_err();
        assert_eq!(step_err.kind, ErrorKind::TooManySteps);
        assert_eq!(machine.live_bytes(), baseline);

        let with_err = machine
            .eval("(with Card { cost: 1 } { missing: 2 })")
            .unwrap_err();
        assert_eq!(with_err.kind, ErrorKind::Runtime);
        assert_eq!(machine.live_bytes(), baseline);
    }
}

#[test]
fn memory_limit_does_not_double_free_shared_values_or_modules() {
    // 再現 1: memory_bytes 上限超過時に共有された大域文字列を二重解放しない。
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(
        Limits {
            steps: 100_000,
            depth: 200,
            memory_bytes: 4000,
        },
        &mut alloc,
    );
    m.eval(r#"(def s "hello")"#).unwrap();
    let big_list = format!("[{}]", vec!["s"; 200].join(" "));
    let err = m.eval(&big_list).unwrap_err();
    assert_eq!(err.kind, ErrorKind::TooMuchMemory);
    assert_eq!(m.eval("s").unwrap(), Value::text("hello"));

    // 再現 2: memory_bytes 上限で失敗した load_module が大域環境を壊さない。
    let mut alloc2 = HeapAllocator;
    let mut m2 = Builder::new(
        Limits {
            steps: 100_000,
            depth: 200,
            memory_bytes: 4096,
        },
        &mut alloc2,
    );
    let big_mod = format!(r#"(def s "{}")"#, "a".repeat(5000));
    let merr = m2.load_module("m", &big_mod).unwrap_err();
    assert_eq!(merr.kind, ErrorKind::TooMuchMemory);
    m2.eval(r#"(def t "xyz")"#).unwrap();
    assert_eq!(m2.eval("t").unwrap(), Value::text("xyz"));
}

#[test]
fn parse_scratch_releases_large_buffers_after_eval_and_error() {
    let mut backing = vec![0u8; 256 * 1024];
    let mut arena = ArenaAllocator::new(&mut backing);
    let mut core = crate::mem::AllocCore::new(&mut arena, 256 * 1024);
    let mut work = crate::mem::WorkArena::new(crate::RegionConfig::new(4 * 1024, 64 * 1024));

    let big_src = "(+ 1 2) ".repeat(200);
    let mark = work.mark(&core);
    let exprs = crate::parser::parse_program_work(&big_src, &mut work, &mut core, false).unwrap();
    assert_eq!(exprs.len(), 200);
    assert!(core.work_bytes() > 0);
    work.rewind(&mut core, mark);
    assert_eq!(core.work_bytes(), 0);

    let bad_deep = "(+ 1 ".repeat(100);
    let mark2 = work.mark(&core);
    assert!(crate::parser::parse_program_work(&bad_deep, &mut work, &mut core, false).is_err());
    work.rewind(&mut core, mark2);
    assert_eq!(core.work_bytes(), 0);

    work.destroy(&mut core);
}

#[test]
fn fn_and_defn_body_last_expr_counts_step() {
    let lim = |steps| Limits {
        steps,
        ..Limits::default()
    };

    assert_eq!(
        eval_with("(defn f (x) x) (f 1)", &[], lim(4))
            .unwrap_err()
            .kind,
        ErrorKind::TooManySteps
    );
    assert_eq!(
        eval_with("(defn f (x) x) (f 1)", &[], lim(5)).unwrap(),
        Value::Int(1)
    );

    assert_eq!(
        eval_with("((fn () 1))", &[], lim(2)).unwrap_err().kind,
        ErrorKind::TooManySteps
    );
    assert_eq!(
        eval_with("((fn () 1))", &[], lim(3)).unwrap(),
        Value::Int(1)
    );

    let chain = "(defn f () 1) (defn g () (f)) (defn f () 2) (g)";
    assert_eq!(
        eval_with(chain, &[], lim(7)).unwrap_err().kind,
        ErrorKind::TooManySteps
    );
    assert_eq!(eval_with(chain, &[], lim(8)).unwrap(), Value::Int(2));
}
