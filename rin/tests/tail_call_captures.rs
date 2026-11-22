//! 末尾呼び出しのフレーム置換でも、呼び出し先と引数の所有権を保つ。
use rin::{ErrorKind, HeapAllocator, Limits, Machine, Value};

#[test]
fn tail_calls_keep_captures_and_arguments_alive() {
    let cases = [
        (
            "(defn t () (let (big [1] f (fn (x) (len big))) (f 0)))",
            "(t)",
            vec![],
            Value::Int(1),
        ),
        (
            "(defn t (big) (let (f (fn (x) (len big))) (f 0)))",
            "(t [1])",
            vec![rin::eval("[1]").unwrap()],
            Value::Int(1),
        ),
        (
            "(defn t () (let (big [1]) ((fn (x) (len big)) 0)))",
            "(t)",
            vec![],
            Value::Int(1),
        ),
        (
            "(defn t () (let (big [1] f (fn (x) (len big))) (+ 0 (f 0))))",
            "(t)",
            vec![],
            Value::Int(1),
        ),
        (
            "(defn t () (let (n 5 f (fn (x) n)) (f 0)))",
            "(t)",
            vec![],
            Value::Int(5),
        ),
        // 引数とキャプチャが同じ値を指し、ヒープ値そのものを返す。
        (
            "(defn t () (let (big [1 2] f (fn (x) (if (== x big) x []))) (f big)))",
            "(t)",
            vec![],
            rin::eval("[1 2]").unwrap(),
        ),
        // 末尾呼び出し先が、さらにキャプチャを持つクロージャを返す。
        (
            "(defn make () (let (big [1 2] f (fn (x) (fn (y) (len big)))) (f 0))) (defn t () (let (g (make)) (g 0)))",
            "(t)",
            vec![],
            Value::Int(2),
        ),
        // map から入った関数の中でも、末尾呼び出し先の寿命を保つ。
        (
            "(defn t () (map (fn (n) (let (big [n] f (fn (x) (len big))) (f 0))) [1 2 3]))",
            "(t)",
            vec![],
            rin::eval("[1 1 1]").unwrap(),
        ),
        // 同一のクロージャへ繰り返し末尾呼び出しする。
        (
            "(defn t () (let (big [1 2] f (fn (self n) (if (== n 0) (len big) (self self (- n 1))))) (f f 1000)))",
            "(t)",
            vec![],
            Value::Int(2),
        ),
        // 呼び出し先と捕捉値を繰り返し交換し、古い値を溜め込まない。
        (
            "(defn hop (n xs) (if (== n 0) xs (let (big [n] f (fn (x) (hop x big))) (f (- n 1))))) (defn t () (hop 1000 []))",
            "(t)",
            vec![],
            rin::eval("[1]").unwrap(),
        ),
    ];
    for (definitions, call, args, expected) in cases {
        for from_host in [false, true] {
            let mut alloc = HeapAllocator;
            let mut machine = Machine::new(
                Limits {
                    memory_bytes: 64 * 1024,
                    ..Limits::default()
                },
                &mut alloc,
            );
            machine.eval(definitions).unwrap();
            let before = machine.live_bytes();
            for _ in 0..3 {
                let value = if from_host {
                    machine.call("t", &args)
                } else {
                    machine.eval(call)
                }
                .unwrap();
                assert_eq!(value, expected, "{definitions}, host={from_host}");
                assert_eq!(
                    machine.live_bytes(),
                    before,
                    "{definitions}, host={from_host}"
                );
            }
        }
    }
}

#[test]
fn tail_call_errors_release_captures() {
    for (body, kind, rule) in [
        ("(/ (len big) 0)", ErrorKind::Runtime, "op.arith"),
        ("(1 big)", ErrorKind::Runtime, "expr.call"),
        ("((fn (a b) a) big)", ErrorKind::Runtime, "expr.call"),
        ("(loop big)", ErrorKind::TooManySteps, "eval.limits"),
        (
            "(map (fn (y) (loop big)) [0])",
            ErrorKind::TooManySteps,
            "eval.limits",
        ),
        ("(range 0 100000)", ErrorKind::TooMuchMemory, "eval.limits"),
    ] {
        for from_host in [false, true] {
            let mut alloc = HeapAllocator;
            let mut machine = Machine::new(
                Limits {
                    steps: 2000,
                    memory_bytes: 64 * 1024,
                    ..Limits::default()
                },
                &mut alloc,
            );
            machine
                .eval(&format!(
                    "(defn loop (x) (loop x)) (defn t () (let (big [1] f (fn (x) {body})) (f 0)))"
                ))
                .unwrap();
            let before = machine.live_bytes();
            let error = if from_host {
                machine.call("t", &[])
            } else {
                machine.eval("(t)")
            }
            .unwrap_err();
            assert_eq!(error.kind, kind);
            assert_eq!(error.rule(), rule);
            assert!(!error.backtrace().is_empty());
            assert_eq!(machine.live_bytes(), before);
            assert_eq!(machine.eval("(+ 1 2)").unwrap(), Value::Int(3));
        }
    }
}
