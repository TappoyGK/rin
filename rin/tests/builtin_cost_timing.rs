//! 手動計測: cargo test -p rin --release --test builtin_cost_timing -- --ignored --nocapture
use rin::{Builder, HeapAllocator, Limits, Value};
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "経過時間は環境に依存するため、計測時だけ実行する"]
fn time_per_step_at_100_and_10000_elements() {
    for n in [100usize, 10_000] {
        let log = (usize::BITS - (n - 1).leading_zeros()) as usize;
        for (label, body, steps) in [
            ("len", "(len xs)", 4),
            ("string-len", "(len text)", n + 4),
            ("contains", "(contains xs -1)", 2 * n + 5),
            ("reverse", "(len (reverse xs))", n + 7),
            ("append", "(len (append xs xs))", 4 * n + 10),
            ("sort", "(len (sort xs))", 2 * n + n * log + 7),
            ("map", "(len (map abs xs))", 3 * n + 8),
            ("unique", "(len (unique xs))", 2 * n + n * (n - 1) / 2 + 7),
            ("equality", "(== xs ys)", 2 * n + 6),
        ] {
            let mut alloc = HeapAllocator;
            let mut m = Builder::new(
                Limits {
                    steps,
                    ..Limits::default()
                },
                &mut alloc,
            );
            let xs = Value::list((0..n).rev().map(|i| Value::Int(i as i64)).collect());
            m.bind("xs", &xs).unwrap();
            m.bind("ys", &xs).unwrap();
            m.bind("text", &Value::str("x".repeat(n))).unwrap();
            m.eval(&format!("(defn run () {body})")).unwrap();
            let mut m = m.freeze().unwrap();
            m.call("run", &[]).unwrap();
            let repeats = if label == "unique" { 3 } else { 100 };
            let start = Instant::now();
            for _ in 0..repeats {
                black_box(m.call("run", &[]).unwrap());
            }
            let ns = start.elapsed().as_nanos() as f64 / repeats as f64;
            println!(
                "{label:10} n={n:5} steps={steps:9} ns/call={ns:12.1} ns/step={:.2}",
                ns / steps as f64
            );
        }
    }
}
