//! cargo run --release --example compile_bench -- small 100000
//! Cases: small, local, defs12000, defs40000, defs:N, prepare:N, locals:N.
//! prepare/locals stop at steps=0 after parsing/compilation; timing includes cleanup.
//! Large cases use a fresh Machine each time; peak is extra live heap bytes during eval.
use rin::{HeapAllocator, Limits, Machine, Value};
use std::{hint::black_box, time::Instant};
mod compile_memory;

fn main() {
    let case = std::env::args().nth(1).unwrap_or_else(|| "small".into());
    let count = match case.as_str() {
        "small" | "local" => 0,
        "defs12000" => 12_000,
        "defs40000" => 40_000,
        _ => case
            .split_once(':')
            .filter(|(kind, _)| matches!(*kind, "defs" | "prepare" | "locals"))
            .map(|(_, n)| n.parse::<i64>().expect("positive size"))
            .filter(|n| *n > 0)
            .expect("unknown case or non-positive size"),
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
        if case.starts_with("locals:") {
            source.push_str("(let (");
            for i in 0..count {
                use std::fmt::Write;
                write!(source, "local{i:08} {i} ").unwrap();
            }
            source.push_str(") ");
            source.push_str(&"local00000000 ".repeat(count as usize));
            source.push(')');
        } else {
            for i in 0..count {
                use std::fmt::Write;
                writeln!(source, "(defn f{i:08} (x) (+ x {i}))").unwrap();
            }
            source.push_str(&format!("(f{:08} 1)", count - 1));
        }
        let prepare = case.starts_with("prepare:") || case.starts_with("locals:");
        let mut peak = 0;
        let mut run = || {
            let mut alloc = HeapAllocator;
            let mut machine = Machine::new(
                Limits {
                    steps: if prepare { 0 } else { usize::MAX },
                    memory_bytes: 64 * 1024 * 1024,
                    ..Limits::default()
                },
                &mut alloc,
            );
            let baseline = compile_memory::start();
            let result = machine.eval(black_box(&source));
            peak = peak.max(compile_memory::peak(baseline));
            if prepare {
                assert_eq!(result.unwrap_err().kind, rin::ErrorKind::TooManySteps);
                assert_eq!(machine.live_bytes(), 0);
            } else {
                assert_eq!(result.unwrap(), Value::Int(count));
            }
        };
        run();
        let start = Instant::now();
        for _ in 0..iterations {
            run();
        }
        println!(
            "{case}: bytes={} peak={peak} ns/op={:.1}",
            source.len(),
            start.elapsed().as_nanos() as f64 / iterations as f64
        );
    }
}
