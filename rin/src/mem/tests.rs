use super::containers::fnv1a_bytes;
use super::*;
use crate::alloc::{ArenaAllocator, HeapAllocator};

#[test]
fn perm_arena_allocates_and_rejects_after_freeze() {
    let mut buf = vec![0u8; 8192];
    let mut arena = ArenaAllocator::new(&mut buf);
    let mut core = AllocCore::new(&mut arena, 4096);
    let mut perm = PermArena::new(RegionConfig::new(512, 2048));

    let s1 = RawStr::from_perm(&mut perm, &mut core, "hello").unwrap();
    let s2 = RawStr::from_perm_cstr(&mut perm, &mut core, "world").unwrap();
    assert_eq!(s1.as_str(), "hello");
    assert_eq!(s2.as_str(), "world");
    // SAFETY: `from_perm_cstr` は末尾に NUL 終端を書き込んでいる。
    assert_eq!(unsafe { *s2.as_ptr().add(s2.len()) }, 0);
    assert!(core.perm_bytes() >= 11);

    perm.freeze();
    assert!(perm.is_frozen());
    assert!(RawStr::from_perm(&mut perm, &mut core, "nope").is_none());

    perm.destroy(&mut core);
    assert_eq!(core.raw_allocated_bytes(), 0);
}

#[test]
fn work_arena_reuses_chunks_across_many_resets() {
    let mut buf = vec![0u8; 32 * 1024];
    let mut arena = ArenaAllocator::new(&mut buf);
    let mut core = AllocCore::new(&mut arena, 16 * 1024);
    let mut work = WorkArena::new(RegionConfig::new(512, 4096));

    // 1 回目でチャンクが確保される。
    let mut v = RawVec::<u64>::new();
    for i in 0..200u64 {
        v.push_work(&mut work, &mut core, i).unwrap();
    }
    assert_eq!(v.len(), 200);
    work.reset(&mut core);
    assert_eq!(core.work_bytes(), 0);

    let used_after_first = core.raw_allocated_bytes();
    assert!(used_after_first > 0);

    // 1000 回繰り返してもアリーナ消費が増えない。
    for _ in 0..1000 {
        let mut v = RawVec::<u64>::new();
        for i in 0..200u64 {
            v.push_work(&mut work, &mut core, i).unwrap();
        }
        assert_eq!(v.as_slice()[199], 199);
        work.reset(&mut core);
        assert_eq!(core.work_bytes(), 0);
    }
    assert_eq!(core.raw_allocated_bytes(), used_after_first);

    work.destroy(&mut core);
    assert_eq!(core.raw_allocated_bytes(), 0);
}

#[test]
fn work_arena_mark_and_rewind() {
    let mut alloc = HeapAllocator;
    let mut core = AllocCore::new(&mut alloc, 64 * 1024);
    let mut work = WorkArena::new(RegionConfig::new(256, 1024));

    let s1 = RawStr::from_work(&mut work, &mut core, "keep").unwrap();
    let mark = work.mark(&core);
    let bytes_at_mark = core.work_bytes();

    for _ in 0..100 {
        let _ = RawStr::from_work(&mut work, &mut core, "temporary-string").unwrap();
    }
    assert!(core.work_bytes() > bytes_at_mark);

    work.rewind(&mut core, mark);
    assert_eq!(core.work_bytes(), bytes_at_mark);
    assert_eq!(s1.as_str(), "keep");

    work.destroy(&mut core);
    assert_eq!(core.raw_allocated_bytes(), 0);
}

#[test]
fn obj_heap_reuses_small_and_large_blocks_without_growing_arena() {
    let mut buf = vec![0u8; 512 * 1024];
    let mut arena = ArenaAllocator::new(&mut buf);
    let mut core = AllocCore::new(&mut arena, 256 * 1024);
    let mut heap = ObjHeap::new(RegionConfig::new(1024, 8192));

    // 小ブロックの確保・解放を 1 回行いウォームアップする。
    let mut blocks = Vec::new();
    for size in [8, 24, 48, 100, 300, 1000] {
        blocks.push(heap.alloc(&mut core, size, 8).unwrap());
    }
    for b in blocks.drain(..) {
        // SAFETY: 直前に `heap.alloc` で確保したブロック。
        unsafe { heap.free(&mut core, b) };
    }
    assert_eq!(core.obj_bytes(), 0);

    // 大ブロック（> 64KB）もウォームアップ。
    let big = heap.alloc(&mut core, 80_000, 8).unwrap();
    // SAFETY: 直前に確保した大ブロック。
    unsafe { heap.free(&mut core, big) };
    assert_eq!(core.obj_bytes(), 0);

    let used_after_warmup = core.raw_allocated_bytes();

    // 何千回繰り返してもアリーナ使用量は 1 バイトも増えない。
    for _ in 0..2000 {
        for size in [8, 24, 48, 100, 300, 1000] {
            let b = heap.alloc(&mut core, size, 8).unwrap();
            // SAFETY: 直前に確保したブロック。
            unsafe { heap.free(&mut core, b) };
        }
        let b = heap.alloc(&mut core, 80_000, 8).unwrap();
        // SAFETY: 直前に確保した大ブロック。
        unsafe { heap.free(&mut core, b) };
        assert_eq!(core.obj_bytes(), 0);
    }
    assert_eq!(core.raw_allocated_bytes(), used_after_warmup);

    heap.destroy(&mut core);
    assert_eq!(core.raw_allocated_bytes(), 0);
}

#[test]
fn raw_map_insert_and_lookup_in_perm_and_work() {
    let mut alloc = HeapAllocator;
    let mut core = AllocCore::new(&mut alloc, 64 * 1024);
    let mut perm = PermArena::new(RegionConfig::new(512, 4096));
    let mut map = RawMap::<RawStr, u32>::new();

    for i in 0..100u32 {
        let key_owned = format!("sym_{i}");
        let key = RawStr::from_perm(&mut perm, &mut core, &key_owned).unwrap();
        let h = fnv1a_bytes(key.as_str().as_bytes());
        let prev = map
            .insert_perm(&mut perm, &mut core, h, key, i, |a, b| {
                a.as_str() == b.as_str()
            })
            .unwrap();
        assert_eq!(prev, None);
    }
    assert_eq!(map.len(), 100);
    for i in 0..100u32 {
        let key_owned = format!("sym_{i}");
        let h = fnv1a_bytes(key_owned.as_bytes());
        let got = map.get_by(h, |k| k.as_str() == key_owned);
        assert_eq!(got, Some(i));
    }

    perm.destroy(&mut core);
    assert_eq!(core.raw_allocated_bytes(), 0);
}
