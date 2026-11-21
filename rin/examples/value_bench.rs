//! cargo run --release --example value_bench で浅い値の処理時間と確保回数を測る。
use rin::{HeapAllocator, Limits, Machine, Schema, Value};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;
fn measure(name: &str, mut f: impl FnMut()) {
    for _ in 0..100 {
        f();
    }
    let before = ALLOCS.load(Ordering::Relaxed);
    let start = Instant::now();
    for _ in 0..20_000 {
        f();
    }
    let elapsed = start.elapsed();
    let count = ALLOCS.load(Ordering::Relaxed) - before;
    println!(
        "{name}: {:.0} ns/op, {:.2} allocs/op",
        elapsed.as_nanos() as f64 / 20_000.0,
        count as f64 / 20_000.0
    );
}
fn main() {
    let mut alloc = HeapAllocator;
    let mut m = Machine::new(Limits::default(), &mut alloc);
    let value = Value::list((0..8).map(Value::Int).collect());
    let other = Value::list((0..8).map(Value::Int).collect());
    let schema = Schema::parse("(list i64)").unwrap();
    measure("eval scalar", || {
        black_box(m.eval("42").unwrap());
    });
    measure("bind list8", || {
        m.bind("x", black_box(&value)).unwrap();
    });
    measure("export list8", || {
        black_box(m.eval("x").unwrap());
    });
    measure("equal list8", || {
        black_box(black_box(&value) == black_box(&other));
    });
    measure("schema list8", || {
        schema.check(black_box(&value)).unwrap();
    });
    measure("display list8", || {
        black_box(black_box(&value).to_string());
    });
    measure("create/drop list8", || {
        black_box(Value::list((0..8).map(Value::Int).collect()));
    });
}
