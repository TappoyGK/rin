//! cargo run --release --example diagnostic_bench -- 2000
//! 引数は各ケースの反復回数。比較する各コミットで同じソースを使う。
use rin::{HeapAllocator, Limits, Machine, Value};
use std::hint::black_box;
use std::time::Instant;

fn measure(label: &str, iterations: usize, mut run: impl FnMut()) {
    for _ in 0..10 {
        run();
    }
    let start = Instant::now();
    for _ in 0..iterations {
        run();
    }
    println!(
        "{label}: {:.1} ns/op",
        start.elapsed().as_nanos() as f64 / iterations as f64
    );
}

fn eval_case(label: &str, source: &str, iterations: usize) {
    let mut alloc = HeapAllocator;
    let mut machine = Machine::new(Limits::default(), &mut alloc);
    machine.eval("(defn identity (x) x)").unwrap();
    measure(label, iterations, || {
        black_box(machine.eval(black_box(source)).unwrap());
    });
}

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(2000)
        .max(1);
    eval_case(
        "short local",
        "(let (damage 7) (* damage damage))",
        iterations,
    );
    let bindings = (0..20)
        .map(|i| format!("local{i} {i} "))
        .collect::<String>();
    let closures = "(fn () (identity local0)) ".repeat(20);
    eval_case(
        "sibling closures",
        &format!("(let ({bindings}) {closures} 0)"),
        iterations,
    );
    eval_case(
        "local abs",
        &format!("(let ({bindings}) {} 0)", "(abs local0) ".repeat(20)),
        iterations,
    );
    eval_case("top abs", &"(abs 1) ".repeat(20), iterations);
    let nested = format!(
        "{}0{} 0",
        "(fn (a b c d e) (identity a) ".repeat(10),
        ")".repeat(10)
    );
    eval_case("nested closures", &nested, iterations);
    let deep_let = format!("{}damage{}", "(let (damage 7) ".repeat(20), ")".repeat(20));
    eval_case("deep let", &deep_let, iterations);
    let bindings = (0..30)
        .map(|i| format!("local{i} {i} "))
        .collect::<String>();
    let matches = "(match 1 (damage damage)) ".repeat(20);
    eval_case(
        "match bindings",
        &format!("(let ({bindings}) {matches} 0)"),
        iterations,
    );

    let large_iterations = (iterations / 20).max(1);
    eval_case(
        "repeated lets",
        &"(let (aa 1 bb 2) (abs aa) (identity bb) (abs aa)) ".repeat(500),
        large_iterations,
    );
    let prefix = "(abs 1) (identity 2) (abs 3) ".repeat(500);
    eval_case("global prefix", &prefix, large_iterations);
    eval_case(
        "sparse local abs",
        &format!("{prefix}(let (xx 1) (abs xx))"),
        large_iterations,
    );
    eval_case(
        "sparse unknown closure",
        &format!("{prefix}(let (xx 1) (fn () xxx))"),
        large_iterations,
    );

    let mut alloc = HeapAllocator;
    let mut machine = Machine::new(Limits::default(), &mut alloc);
    for i in 0..500 {
        machine
            .bind(&format!("{}{:03}", "z".repeat(1997), i), &Value::Int(0))
            .unwrap();
    }
    let query = "a".repeat(2000);
    measure("distant names", (iterations / 10).max(1), || {
        black_box(machine.call(black_box(&query), &[]).unwrap_err());
    });
}
