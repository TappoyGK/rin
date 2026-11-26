//! システムヒープからの確保回数を計測・禁止する検証テスト基盤。
//!
//! #19 の各段階の進捗に合わせて、システムヒープ確保回数が 0 であることを検証する。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAllocator;

thread_local! {
    static COUNTING_ENABLED: Cell<bool> = const { Cell::new(false) };
    static FORBID_HEAP: Cell<bool> = const { Cell::new(false) };
    static THREAD_ALLOCS: Cell<usize> = const { Cell::new(0) };
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

impl CountingAllocator {
    #[inline]
    fn on_alloc() -> bool {
        if FORBID_HEAP.with(|f| f.get()) {
            return false;
        }
        if COUNTING_ENABLED.with(|c| c.get()) {
            THREAD_ALLOCS.with(|n| n.set(n.get() + 1));
        }
        true
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !Self::on_alloc() {
            return std::ptr::null_mut();
        }
        // SAFETY: システムアロケータへそのまま委譲する。
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !Self::on_alloc() {
            return std::ptr::null_mut();
        }
        // SAFETY: システムアロケータへそのまま委譲する。
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: システムアロケータへそのまま委譲する。
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if !Self::on_alloc() {
            return std::ptr::null_mut();
        }
        // SAFETY: システムアロケータへそのまま委譲する。
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// 渡されたクロージャ実行中のシステムヒープ確保回数を返す。
fn measure_heap_allocs<R>(f: impl FnOnce() -> R) -> (R, usize) {
    THREAD_ALLOCS.with(|n| n.set(0));
    COUNTING_ENABLED.with(|c| c.set(true));
    let res = f();
    COUNTING_ENABLED.with(|c| c.set(false));
    let count = THREAD_ALLOCS.with(|n| n.get());
    (res, count)
}

#[test]
fn counting_allocator_detects_std_heap_allocations() {
    let (_, count) = measure_heap_allocs(|| {
        let v: Vec<u8> = vec![1, 2, 3, 4];
        std::hint::black_box(v);
    });
    assert!(count >= 1);
}

#[test]
fn arena_allocator_itself_uses_zero_system_heap() {
    let mut backing = vec![0u8; 4096];
    let (used, count) = measure_heap_allocs(|| {
        let mut arena = rin::ArenaAllocator::new(&mut backing);
        let p = rin::Allocator::alloc(&mut arena, Layout::from_size_align(128, 8).unwrap());
        assert!(p.is_some());
        arena.used()
    });
    assert_eq!(count, 0);
    assert_eq!(used, 128);
}
