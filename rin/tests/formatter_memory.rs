use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn add(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(live, Relaxed);
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            add(layout.size());
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(ptr, layout, size) };
        if !result.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
            add(size);
        }
        result
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn formatter_peak_memory() {
    for n in [1600, 16000] {
        let mut source = String::new();
        for i in 0..n {
            source.push_str(&format!(
                "; c{i}\n(defn f{i} (x y)\n  (if (< x y) (+ x (* y {i})) (- x y))) ; t\n\n"
            ));
        }
        measure(&format!("definitions={n}"), &source);
    }
    measure("call", &format!("(f {})", "x ".repeat(100_000)));
    measure("list", &format!("[{}]", "1 ".repeat(100_000)));
    measure(
        "nested",
        &format!("{}1{}\n", "(when true ".repeat(200), ")".repeat(200)).repeat(100),
    );
}

fn measure(label: &str, source: &str) {
    let baseline = LIVE.load(Relaxed);
    PEAK.store(baseline, Relaxed);
    let start = std::time::Instant::now();
    let output = rin::fmt::format(source).unwrap();
    let peak = PEAK.load(Relaxed) - baseline;
    eprintln!(
        "{label} input={} peak={peak} elapsed={:?}",
        source.len(),
        start.elapsed()
    );
    assert!(!output.is_empty());
    // 元の実装（約270～320倍）への退行を検出する。実行時間は環境差があるため制限しない。
    assert!(peak < source.len() * 220, "{label}: peak {peak}");
}
