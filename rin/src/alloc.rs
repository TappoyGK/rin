//! ランタイムプールが利用する外部メモリ割り当て境界とアリーナ実装を定義する。

use std::alloc::Layout;
use std::marker::PhantomData;
use std::ptr::NonNull;

/// Rin のランタイムプールが利用する外部メモリアロケータ境界。
///
/// ホスト（アリーナベースのランタイムなど）はこのトレイトを実装して
/// メモリスラブの確保先を制御できる。
///
/// # Safety
/// このトレイトを実装する型は、以下の契約をすべて満たさなければならない。
///
/// - **`alloc(layout)` の成功時（`Some(ptr)`）**：
///   - `layout.size() > 0` のとき、`ptr` は `layout.align()` に整列した、少なくとも `layout.size()` バイトの読み書き可能な有効領域の先頭を指すこと。
///   - 同時に生存している（未解放の）ほかの確保領域とメモリ範囲が重複しないこと。
///   - 返された領域は、対応する `dealloc(ptr, layout)` が呼ばれるかアロケータ自身が破棄されるまで有効かつ再利用されないこと。アリーナ実装で個別解放を省略する場合も、生存中の領域を早期に再利用してはならない。
///   - `layout.size() == 0` のとき、成功時は `layout.align()` に整列した非 NULL ポインタ（例：`std::ptr::without_provenance_mut(layout.align())`）を返し、領域への読み書きを行わない前提で扱えること。
/// - **`dealloc(ptr, layout)`**：
///   - 呼び出し側が「`self.alloc(layout)` が返した未解放のポインタと同じ `layout`」を渡したとき、対応する確保方式に従って安全に解放（またはアリーナの場合は無操作）すること。
///   - `layout.size() == 0` のときは何もせず安全に戻ること。
///
/// 呼び出し側は、`dealloc` に渡す `ptr` が `self.alloc(layout)` が返した未解放のポインタであり、`layout` が確保時と同じであること、また `layout.size() == 0` で得たポインタを通じて読み書きしないことを保証する。
///
/// ```compile_fail,E0200
/// use std::alloc::Layout;
/// use std::ptr::NonNull;
/// use rin::Allocator;
///
/// struct Sample;
/// // `unsafe impl` なしでは実装できない。
/// impl Allocator for Sample {
///     fn alloc(&mut self, _: Layout) -> Option<NonNull<u8>> { None }
///     unsafe fn dealloc(&mut self, _: NonNull<u8>, _: Layout) {}
/// }
/// ```
///
/// ```rust
/// use std::alloc::Layout;
/// use std::ptr::NonNull;
/// use rin::Allocator;
///
/// struct Sample;
/// // SAFETY: 常に `None` を返すため確保領域は発生せず、`dealloc` が呼ばれることもない。
/// unsafe impl Allocator for Sample {
///     fn alloc(&mut self, _: Layout) -> Option<NonNull<u8>> { None }
///     unsafe fn dealloc(&mut self, _: NonNull<u8>, _: Layout) {}
/// }
/// ```
pub unsafe trait Allocator {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>>;

    /// 以前に確保したブロックを解放する。
    ///
    /// アリーナ実装では個別解放を無視し、スコープ終了時に一括回収してもよい。
    ///
    /// # Safety
    /// `ptr` は `self.alloc(layout)` が返した未解放のポインタであり、`layout` は確保時と同じでなければならない。
    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout);
}

/// Rust の標準グローバルヒープを使う既定のアロケータ。
#[derive(Debug, Default, Clone, Copy)]
pub struct HeapAllocator;

// SAFETY:
// - `layout.size() == 0` では `layout.align()` に整列した非 NULL ポインタを返し、`dealloc` は無操作とする。
// - `layout.size() > 0` では `std::alloc::alloc(layout)` へ委譲し、要求サイズ・アラインメントを満たす
//   互いに重複しない読み書き可能領域を返し、`std::alloc::dealloc(ptr, layout)` まで有効に保つ。
unsafe impl Allocator for HeapAllocator {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        if layout.size() == 0 {
            return NonNull::new(std::ptr::without_provenance_mut(layout.align()));
        }
        // SAFETY: 直前で `layout.size() > 0` を確認済み。
        let ptr = unsafe { std::alloc::alloc(layout) };
        NonNull::new(ptr)
    }

    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() == 0 {
            return;
        }
        // SAFETY: 呼び出し側が `std::alloc::alloc(layout)` 由来のポインタであることを保証する。
        unsafe { std::alloc::dealloc(ptr.as_ptr(), layout) };
    }
}

/// 呼び出し側が渡したバイトスライス上で動作するバンプポインタ型アリーナ。
///
/// 個別の `dealloc` は何もしない。スライスの破棄またはリセット時に一括解放する。
pub struct ArenaAllocator<'a> {
    buf: &'a mut [u8],
    offset: usize,
}

impl<'a> ArenaAllocator<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, offset: 0 }
    }

    pub fn used(&self) -> usize {
        self.offset
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }
}

// SAFETY:
// - `layout.size() == 0` では `layout.align()` に整列した非 NULL ポインタを返し、`offset` を進めない。
// - `layout.size() > 0` では `self.buf` 内の現在実アドレスを `layout.align()` に切り上げた先頭から
//   `layout.size()` バイトを単調増加する `self.offset` で切り出すため、整列・有効性・非重複を満たし、
//   `ArenaAllocator` の破棄まで再利用されない。`dealloc` は無操作。
unsafe impl Allocator for ArenaAllocator<'_> {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        let align = layout.align();
        if layout.size() == 0 {
            return NonNull::new(std::ptr::without_provenance_mut(align));
        }
        let base = self.buf.as_mut_ptr();
        let base_addr = base as usize;
        let cur_addr = base_addr.checked_add(self.offset)?;
        let aligned_addr = cur_addr.checked_add(align - 1)? & !(align - 1);
        let aligned_offset = aligned_addr - base_addr;
        let end = aligned_offset.checked_add(layout.size())?;
        if end > self.buf.len() {
            return None;
        }
        self.offset = end;
        // SAFETY: `aligned_offset < end <= self.buf.len()` が成り立つ。
        let ptr = unsafe { base.add(aligned_offset) };
        debug_assert_eq!((ptr as usize) & (align - 1), 0);
        NonNull::new(ptr)
    }

    unsafe fn dealloc(&mut self, _ptr: NonNull<u8>, _layout: Layout) {
        // アリーナは背後バッファの破棄またはリセット時に一括回収するため個別解放は不要。
    }
}

/// `Allocator` から確保した連続領域バッファ。
pub(crate) struct AllocBuf<T> {
    ptr: NonNull<T>,
    cap: usize,
    _marker: PhantomData<T>,
}

impl<T> AllocBuf<T> {
    pub fn from_raw(ptr: NonNull<T>, cap: usize) -> Self {
        Self {
            ptr,
            cap,
            _marker: PhantomData,
        }
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn into_raw_ptr(self) -> NonNull<T> {
        self.ptr
    }

    #[inline(always)]
    pub fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    #[inline(always)]
    pub fn as_slice(&self, len: usize) -> &[T] {
        debug_assert!(len <= self.cap);
        if len == 0 {
            return &[];
        }
        // SAFETY: `len <= self.cap` かつ `0..len` が初期化済み。
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), len) }
    }

    /// 未初期化の位置 `idx` に `value` を書き込む。
    ///
    /// # Safety
    /// `idx < self.cap` かつ位置 `idx` が未初期化でなければならない。
    pub unsafe fn write_at(&mut self, idx: usize, value: T) {
        debug_assert!(idx < self.cap);
        // SAFETY: 呼び出し側が `idx < self.cap` と未初期化を保証する。
        unsafe { self.ptr.as_ptr().add(idx).write(value) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_aligns_from_unaligned_slice_start() {
        let mut raw = [0u8; 256];
        for align in [1usize, 2, 4, 8, 16, 32] {
            for shift in 0..align {
                let mut arena = ArenaAllocator::new(&mut raw[shift..]);
                let layout = Layout::from_size_align(align, align).unwrap();
                let ptr = arena.alloc(layout).expect("allocation should succeed");
                assert_eq!(
                    (ptr.as_ptr() as usize) % align,
                    0,
                    "align={align} shift={shift}"
                );
            }
        }
    }

    #[test]
    fn arena_multiple_allocations_are_aligned_and_disjoint() {
        let mut raw = [0u8; 512];
        let start = (0..16)
            .find(|&i| (raw.as_ptr() as usize + i) % 16 == 1)
            .unwrap();
        let mut arena = ArenaAllocator::new(&mut raw[start..]);

        let specs = [
            (1usize, 1usize, 0x11u8),
            (8, 8, 0x22),
            (3, 1, 0x33),
            (16, 16, 0x44),
            (4, 4, 0x55),
            (32, 32, 0x66),
            (2, 2, 0x77),
        ];

        let mut regions = Vec::new();
        for &(size, align, fill) in &specs {
            let layout = Layout::from_size_align(size, align).unwrap();
            let ptr = arena.alloc(layout).expect("allocation should succeed");
            assert_eq!((ptr.as_ptr() as usize) % align, 0);
            // SAFETY: 確保した `size` バイトは有効かつ互いに重複しない。
            unsafe { std::ptr::write_bytes(ptr.as_ptr(), fill, size) };
            regions.push((ptr.as_ptr() as usize, size, fill));
        }

        // 先に書いた内容が後続の確保で破壊されていないことと、領域が重複していないことを確認する。
        for (i, &(addr, size, fill)) in regions.iter().enumerate() {
            // SAFETY: テスト内の `raw` バッファ上の有効な領域。
            let slice = unsafe { std::slice::from_raw_parts(addr as *const u8, size) };
            assert!(slice.iter().all(|&b| b == fill));
            for &(other_addr, other_size, _) in &regions[i + 1..] {
                assert!(addr + size <= other_addr || other_addr + other_size <= addr);
            }
        }
    }

    #[test]
    fn arena_padding_exhaustion_preserves_used() {
        let mut raw = [0u8; 64];
        let start = (0..8)
            .find(|&i| (raw.as_ptr() as usize + i) % 8 == 1)
            .unwrap();
        // 先頭アドレス % 8 == 1 なので、8 バイト整列に 7 バイトの余白が要る。
        // 長さ 14 バイトでは 7 + 8 = 15 > 14 となり失敗する。
        let mut arena = ArenaAllocator::new(&mut raw[start..start + 14]);
        let layout8 = Layout::from_size_align(8, 8).unwrap();
        assert!(arena.alloc(layout8).is_none());
        assert_eq!(arena.used(), 0);

        // 1 バイト確保したあとでも、余白込みで収まらない要求は失敗し `used()` を変えない。
        let layout1 = Layout::from_size_align(1, 1).unwrap();
        assert!(arena.alloc(layout1).is_some());
        assert_eq!(arena.used(), 1);
        assert!(arena.alloc(layout8).is_none());
        assert_eq!(arena.used(), 1);

        // ちょうど収まる 15 バイトなら成功する（余白 7 + 本体 8 = 15）。
        let mut exact = ArenaAllocator::new(&mut raw[start..start + 15]);
        let ptr = exact.alloc(layout8).expect("exact fit should succeed");
        assert_eq!((ptr.as_ptr() as usize) % 8, 0);
        assert_eq!(exact.used(), 15);
    }

    #[test]
    fn heap_zero_size_returns_aligned_pointer() {
        let mut heap = HeapAllocator;
        for align in [1usize, 2, 4, 8, 16, 4096] {
            let layout = Layout::from_size_align(0, align).unwrap();
            let ptr = heap.alloc(layout).expect("zero-size alloc should succeed");
            assert_eq!((ptr.as_ptr() as usize) % align, 0, "align={align}");
            // SAFETY: 直前に `heap.alloc(layout)` が返したポインタと同じ `layout`。
            unsafe { heap.dealloc(ptr, layout) };
        }
    }

    #[test]
    fn arena_zero_size_returns_aligned_pointer_without_advancing_used() {
        let mut raw = [0u8; 64];
        let mut arena = ArenaAllocator::new(&mut raw);
        for align in [1usize, 2, 4, 8, 16, 4096] {
            let layout = Layout::from_size_align(0, align).unwrap();
            let ptr = arena.alloc(layout).expect("zero-size alloc should succeed");
            assert_eq!((ptr.as_ptr() as usize) % align, 0, "align={align}");
            assert_eq!(arena.used(), 0);
        }
    }

    #[test]
    fn arena_huge_size_preserves_used() {
        let mut raw = [0u8; 64];
        let start = (0..8)
            .find(|&i| (raw.as_ptr() as usize + i) % 8 == 1)
            .unwrap();
        let mut arena = ArenaAllocator::new(&mut raw[start..]);
        let huge = Layout::from_size_align(isize::MAX as usize - 7, 8).unwrap();
        assert!(arena.alloc(huge).is_none());
        assert_eq!(arena.used(), 0);
    }

    #[test]
    fn builder_works_on_unaligned_arena_slice() {
        let mut raw = vec![0u8; 128 * 1024];
        let start = (0..16)
            .find(|&i| (raw.as_ptr() as usize + i) % 16 == 1)
            .unwrap();
        let mut arena = ArenaAllocator::new(&mut raw[start..]);
        let mut builder = crate::Builder::new(crate::Limits::default(), &mut arena);
        let val = builder
            .eval_ref("(fold + 0 (map (fn (x) (* x 2)) [1 2 3 4]))")
            .expect("eval should succeed on unaligned arena");
        assert_eq!(val.as_int(), Some(20));
    }
}
