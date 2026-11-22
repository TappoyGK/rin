//! 遠い候補が増えても距離計算用の確保回数が増えないことを検証する。
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

#[test]
fn distant_candidates_do_not_allocate_dp_rows() {
    let mut alloc = HeapAllocator;
    let mut machine = Machine::new(Limits::default(), &mut alloc);
    let query = "a".repeat(2000);
    let measure = |machine: &mut Machine<'_>| {
        COUNT.set(Some(0));
        let error = machine.call(&query, &[]).unwrap_err();
        let count = COUNT.replace(None).unwrap();
        assert!(!error.message.contains("did you mean"));
        count
    };
    let empty = measure(&mut machine);
    for i in 0..500 {
        machine
            .bind(&format!("{}{:03}", "z".repeat(1997), i), &Value::Int(0))
            .unwrap();
    }
    assert_eq!(measure(&mut machine), empty);
}

fn eval_allocations(source: &str) -> usize {
    let mut alloc = HeapAllocator;
    let mut machine = Machine::new(Limits::default(), &mut alloc);
    for _ in 0..5 {
        machine.eval(source).unwrap();
    }
    COUNT.set(Some(0));
    let result = machine.eval(source);
    let count = COUNT.replace(None).unwrap();
    result.unwrap();
    count
}

#[test]
fn successful_hint_metadata_has_constant_allocation_count() {
    for width in [20, 200, 2000] {
        let bindings = (0..width)
            .map(|i| format!("local{i} {i} "))
            .collect::<String>();
        for closures in [false, true] {
            let body = if closures {
                "(fn () (CALLEE local0)) "
            } else {
                "(CALLEE local0) "
            }
            .repeat(width);
            let source = format!("(let ({bindings}) {body} 0)");
            let short = eval_allocations(&source.replace("CALLEE", "-"));
            let long = eval_allocations(&source.replace("CALLEE", "abs"));
            assert!(
                long <= short + 5,
                "width={width}, closures={closures}: {short} -> {long}"
            );
        }
    }
}

#[test]
fn no_locals_need_no_hint_allocations() {
    let body = "(CALLEE 1) ".repeat(200);
    assert_eq!(
        eval_allocations(&body.replace("CALLEE", "abs")),
        eval_allocations(&body.replace("CALLEE", "-"))
    );
}
