//! cargo run --release --example diagnostic_allocations
//! 同一ソースを各コミットで実行し、6 回目の呼び出しの alloc/realloc 回数を比較する。
use rin::{HeapAllocator, Limits, Machine, Value};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;
thread_local! {
    static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
}
fn record() {
    let _ = COUNT.try_with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record();
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

use std::hint::black_box;

fn measure(label: &str, _: usize, mut run: impl FnMut()) {
    for _ in 0..5 {
        run();
    }
    COUNT.set(Some(0));
    run();
    let count = COUNT.replace(None).unwrap();
    println!("{label}: {count} allocations");
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
