//! 通常実行の速度と、2 MiBスタックの余裕を比較する手動ベンチマーク。
//! cargo run -p rin --release --example cost_bench
//! cargo run -p rin --example cost_bench -- stack 300
use rin::{Builder, HeapAllocator, Limits, Machine, Value};
use std::{collections::BTreeMap, hint::black_box, time::Instant};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "stack") {
        let n = args[2].parse::<i64>().unwrap();
        std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(move || {
            let mut alloc = HeapAllocator;
            let mut m = Builder::new(Limits { steps: usize::MAX, depth: usize::MAX, ..Limits::default() }, &mut alloc);
            m.eval("(defn mdeep (n) (if (== n 0) 0 (+ 1 (fold (fn (acc x) (mdeep (- n 1))) 0 [1]))))").unwrap();
            let mut m = m.freeze().unwrap();
            assert_eq!(m.call("mdeep", &[Value::Int(n)]).unwrap(), Value::Int(n));
        }).unwrap().join().unwrap();
        return;
    }
    let mut alloc = HeapAllocator;
    let mut m = Builder::new(
        Limits {
            steps: usize::MAX,
            ..Limits::default()
        },
        &mut alloc,
    );
    m.eval("(defn identity (x) x) (defn direct (x) (+ 1 (identity x))) (defn recur (n) (if (== n 0) 0 (+ 1 (recur (- n 1))))) (defn tail (n acc) (if (== n 0) acc (tail (- n 1) (+ acc 1)))) (defn fib (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))").unwrap();
    let mut m = m.freeze().unwrap();
    for (name, args, repeats) in [
        ("direct", vec![Value::Int(7)], 200_000),
        ("recur", vec![Value::Int(100)], 2_000),
        ("tail", vec![Value::Int(1000), Value::Int(0)], 500),
        ("fib", vec![Value::Int(10)], 2_000),
    ] {
        bench(&mut m, name, name, &args, repeats);
    }
    for n in [10usize, 10_000] {
        let xs = Value::list((0..n).map(|i| Value::Int(i as i64)).collect());
        let s = Value::structure(
            "S",
            (0..n)
                .map(|i| (format!("f{i}"), Value::Int(i as i64)))
                .collect::<BTreeMap<_, _>>(),
        );
        for (name, body) in [
            ("len", "(len xs)"),
            ("contains", "(contains xs 9)"),
            ("equal-list", "(== xs ys)"),
            ("equal-struct", "(== s t)"),
            ("fold", "(fold + 0 xs)"),
            ("unique", "(len (unique xs))"),
        ] {
            let mut alloc = HeapAllocator;
            let mut b = Builder::new(
                Limits {
                    steps: usize::MAX,
                    ..Limits::default()
                },
                &mut alloc,
            );
            b.bind("xs", &xs).unwrap();
            b.bind("ys", &xs).unwrap();
            b.bind("s", &s).unwrap();
            b.bind("t", &s).unwrap();
            b.eval(&format!("(defn run () {body})")).unwrap();
            let mut m = b.freeze().unwrap();
            let repeats = if n == 10 {
                100_000
            } else if name == "unique" {
                3
            } else {
                100
            };
            bench(&mut m, &format!("{name}/{n}"), "run", &[], repeats);
        }
    }
}
fn bench(m: &mut Machine<'_>, label: &str, name: &str, args: &[Value], repeats: usize) {
    for _ in 0..10 {
        black_box(m.call(name, args).unwrap());
    }
    let start = Instant::now();
    for _ in 0..repeats {
        black_box(m.call(black_box(name), black_box(args)).unwrap());
    }
    println!(
        "{label} {:.1}",
        start.elapsed().as_nanos() as f64 / repeats as f64
    );
}
