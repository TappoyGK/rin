//! cargo run --release --example call_bench -- 10000
//! 読み込み済み関数の呼び出し経路を比較する。各ケースはコンパイル時間を含まない。
use rin::{HeapAllocator, Limits, Machine, Value};
use std::{hint::black_box, time::Instant};

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(10_000)
        .max(1);
    let mut alloc = HeapAllocator;
    let mut machine = Machine::new(Limits::default(), &mut alloc);
    machine
        .eval(
            r#"
        (defn identity (x) x)
        (defn direct (x) (+ 1 (identity x)))
        (defn recur (n) (if (== n 0) 0 (+ 1 (recur (- n 1)))))
        (defn tail (n acc) (if (== n 0) acc (tail (- n 1) (+ acc 1))))
        (defn fib (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
        (def xs (range 0 100))
        (defn mapped () (len (map identity xs)))
        (defn folded () (fold (fn (acc x) (+ acc x)) 0 xs))
    "#,
        )
        .unwrap();
    for (name, args, expected) in [
        ("identity", vec![Value::Int(7)], 7),
        ("direct", vec![Value::Int(7)], 8),
        ("recur", vec![Value::Int(100)], 100),
        ("tail", vec![Value::Int(100), Value::Int(0)], 100),
        ("fib", vec![Value::Int(10)], 55),
        ("mapped", vec![], 100),
        ("folded", vec![], 4950),
    ] {
        for _ in 0..20 {
            assert_eq!(machine.call(name, &args).unwrap(), Value::Int(expected));
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
}
