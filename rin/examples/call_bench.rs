//! cargo run --release --example call_bench -- 10000
//! 第2引数にケース名を指定できる。eval_fib 以外は読み込み済み関数を呼ぶ。
use rin::{Builder, HeapAllocator, Limits, Value};
use std::{hint::black_box, time::Instant};

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(10_000)
        .max(1);
    let selected = std::env::args().nth(2);
    let mut alloc = HeapAllocator;
    let mut machine = Builder::new(Limits::default(), &mut alloc);
    machine
        .eval(
            r#"
        (defn identity (x) x)
        (defn direct (x) (+ 1 (identity x)))
        (defn recur (n) (if (== n 0) 0 (+ 1 (recur (- n 1)))))
        (defn deep (n) (if (== n 0) 0 (+ 1 (deep (- n 1)))))
        (defn closure_mapped (bias) (len (map (fn (x) (+ bias x)) xs)))
        (defn tail (n acc) (if (== n 0) acc (tail (- n 1) (+ acc 1))))
        (defn fib (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
        (def xs (range 0 100))
        (defn mapped () (len (map identity xs)))
        (defn cb_filter () (len (filter (fn (x) (< x 50)) xs)))
        (defn cb_any () (any (fn (x) (== x 99)) xs))
        (defn cb_all () (all (fn (x) (< x 1000)) xs))
        (defn cb_find () (find (fn (x) (> x 98)) xs))
        (defn cb_sort_by () (len (sort_by (fn (x) (- 100 x)) xs)))
        (defn cb_flat_map () (len (flat_map (fn (x) [x]) xs)))
        (defn folded () (fold (fn (acc x) (+ acc x)) 0 xs))
    "#,
        )
        .unwrap();
    let mut machine = machine.freeze().unwrap();
    for (name, args, expected) in [
        ("identity", vec![Value::Int(7)], Value::Int(7)),
        ("direct", vec![Value::Int(7)], Value::Int(8)),
        ("recur", vec![Value::Int(100)], Value::Int(100)),
        ("deep", vec![Value::Int(199)], Value::Int(199)),
        ("closure_mapped", vec![Value::Int(7)], Value::Int(100)),
        (
            "tail",
            vec![Value::Int(100), Value::Int(0)],
            Value::Int(100),
        ),
        ("fib", vec![Value::Int(10)], Value::Int(55)),
        ("mapped", vec![], Value::Int(100)),
        ("folded", vec![], Value::Int(4950)),
        ("cb_filter", vec![], Value::Int(50)),
        ("cb_any", vec![], Value::Bool(true)),
        ("cb_all", vec![], Value::Bool(true)),
        ("cb_find", vec![], Value::some(Value::Int(99))),
        ("cb_sort_by", vec![], Value::Int(100)),
        ("cb_flat_map", vec![], Value::Int(100)),
    ] {
        if selected.as_ref().is_some_and(|wanted| wanted != name) {
            continue;
        }
        for _ in 0..1000 {
            assert_eq!(machine.call(name, &args).unwrap(), expected);
        }
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(machine.call(black_box(name), black_box(&args)).unwrap());
        }
        println!(
            "{name}: {:.1} ns/op",
            start.elapsed().as_nanos() as f64 / iterations as f64
        );
    }
    if selected.as_ref().is_none_or(|wanted| wanted == "eval_fib") {
        let source = "(defn ef (n) (if (< n 2) n (+ (ef (- n 1)) (ef (- n 2))))) (ef 12)";
        let mut alloc = HeapAllocator;
        let mut builder = Builder::new(Limits::default(), &mut alloc);
        for _ in 0..1000 {
            assert_eq!(builder.eval(source).unwrap(), Value::Int(144));
        }
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(builder.eval(black_box(source)).unwrap());
        }
        println!(
            "eval_fib: {:.1} ns/op",
            start.elapsed().as_nanos() as f64 / iterations as f64
        );
    }
}
