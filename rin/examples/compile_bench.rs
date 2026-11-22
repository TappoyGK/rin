//! cargo run --release --example compile_bench -- small 100000
//! Cases: small, local, defs12000, defs40000. Large cases use a fresh Machine each time.
use rin::{HeapAllocator, Limits, Machine, Value};
use std::{hint::black_box, time::Instant};

fn main() {
    let case = std::env::args().nth(1).unwrap_or_else(|| "small".into());
    let count = match case.as_str() {
        "small" | "local" => 0,
        "defs12000" => 12_000,
        "defs40000" => 40_000,
        _ => panic!("unknown case: {case}"),
    };
    let iterations = std::env::args()
        .nth(2)
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(if count == 0 { 100_000 } else { 10 })
        .max(1);
    if count == 0 {
        let mut alloc = HeapAllocator;
        let mut machine = Machine::new(Limits::default(), &mut alloc);
        machine.eval("(defn sq (x) (* x x))").unwrap();
        let source = if case == "small" {
            "(sq 7)"
        } else {
            "(let (x 7) (* x x))"
        };
        for _ in 0..1000 {
            assert_eq!(machine.eval(source).unwrap(), Value::Int(49));
        }
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(machine.eval(black_box(source)).unwrap());
        }
        println!(
            "{case}: {:.1} ns/op",
            start.elapsed().as_nanos() as f64 / iterations as f64
        );
    } else {
        let mut source = String::new();
        for i in 0..count {
            use std::fmt::Write;
            writeln!(source, "(defn f{i} (x) (+ x {i}))").unwrap();
        }
        source.push_str(&format!("(f{} 1)", count - 1));
        let run = || {
            let mut alloc = HeapAllocator;
            let mut machine = Machine::new(
                Limits {
                    steps: 1_000_000,
                    memory_bytes: 64 * 1024 * 1024,
                    ..Limits::default()
                },
                &mut alloc,
            );
            assert_eq!(machine.eval(black_box(&source)).unwrap(), Value::Int(count));
        };
        run();
        let start = Instant::now();
        for _ in 0..iterations {
            run();
        }
        println!(
            "{case}: {:.1} ns/op",
            start.elapsed().as_nanos() as f64 / iterations as f64
        );
    }
}
