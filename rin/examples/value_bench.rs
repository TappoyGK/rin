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
fn measure(name: &str, f: impl FnMut()) {
    measure_n(name, 20000, f);
}
fn measure_n(name: &str, n: usize, mut f: impl FnMut()) {
    for _ in 0..100 {
        f();
    }
    let before = ALLOCS.load(Ordering::Relaxed);
    let start = Instant::now();
    for _ in 0..n {
        f();
    }
    let elapsed = start.elapsed();
    let count = ALLOCS.load(Ordering::Relaxed) - before;
    println!(
        "{name}: {:.2} ns/op, {:.2} allocs/op",
        elapsed.as_nanos() as f64 / n as f64,
        count as f64 / n as f64
    );
}
fn main() {
    if std::env::args().any(|arg| arg == "--extended") {
        extended();
        return;
    }
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

fn make_case(case: &str) -> Value {
    use std::collections::BTreeMap;
    use std::rc::Rc;
    fn tree(depth: usize, width: usize) -> Value {
        if depth == 0 {
            return Value::structure("Leaf", BTreeMap::from([("value".into(), Value::Int(1))]));
        }
        Value::structure(
            "Node",
            BTreeMap::from([(
                "children".into(),
                Value::list((0..width).map(|_| tree(depth - 1, width)).collect()),
            )]),
        )
    }
    match case {
        "list8" => Value::list((0..8).map(Value::Int).collect()),
        "list1000" => Value::list((0..1000).map(Value::Int).collect()),
        "arr1000" => Value::array(
            (0..1000)
                .map(|i| Value::tuple(vec![Value::Int(i), Value::Int(i + 1)]))
                .collect(),
        ),
        "tree4x4" => tree(4, 4),
        "tree8x2" => tree(8, 2),
        "struct8" => Value::structure(
            "S",
            BTreeMap::from([
                ("a".into(), Value::Int(1)),
                ("b".into(), Value::Float(2.0)),
                ("c".into(), Value::str("abc")),
                ("d".into(), Value::Bool(true)),
                ("e".into(), Value::list(vec![Value::Int(1), Value::Int(2)])),
                ("f".into(), Value::Some(Rc::new(Value::Int(3)))),
                ("g".into(), Value::None),
                ("h".into(), Value::str("xyz")),
            ]),
        ),
        _ => unreachable!(),
    }
}
fn extended() {
    for case in [
        "list8", "list1000", "struct8", "tree4x4", "tree8x2", "arr1000",
    ] {
        let n = if matches!(case, "list8" | "struct8") {
            20_000
        } else {
            2_000
        };
        let v = make_case(case);
        let other = make_case(case);
        let mut alloc = HeapAllocator;
        let mut m = Machine::new(Limits::default(), &mut alloc);
        m.eval("(defn id (x) x)").unwrap();
        m.bind("x", &v).unwrap();
        measure_n(&format!("{case} bind"), n, || {
            m.bind("x", black_box(&v)).unwrap();
        });
        measure_n(&format!("{case} export"), n, || {
            black_box(m.eval("x").unwrap());
        });
        measure_n(&format!("{case} call"), n, || {
            black_box(m.call("id", std::slice::from_ref(&v)).unwrap());
        });
        measure_n(&format!("{case} eq"), n, || {
            black_box(black_box(&v) == black_box(&other));
        });
        measure_n(&format!("{case} display"), n, || {
            black_box(v.to_string());
        });
        measure_n(&format!("{case} debug"), n, || {
            black_box(format!("{v:?}"));
        });
        measure_n(&format!("{case} create/drop"), n, || {
            black_box(make_case(case));
        });
        measure_n(&format!("{case} shared clone/drop"), 200_000, || {
            black_box(black_box(&v).clone());
        });
    }
    let mut alloc = HeapAllocator;
    let mut m = Machine::new(Limits::default(), &mut alloc);
    m.eval("(defn dedup () (unique (range 0 1000)))").unwrap();
    measure_n("VM unique1000", 50, || {
        black_box(m.call("dedup", &[]).unwrap());
    });
}
