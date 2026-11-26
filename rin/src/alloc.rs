//! ランタイムプールが利用する外部メモリ割り当て境界とアリーナ実装を定義する。

use std::alloc::Layout;
use std::marker::PhantomData;
use std::ptr::NonNull;

/// Rin のランタイムプールが利用する外部メモリアロケータ境界。
///
/// ホスト（アリーナベースのランタイムなど）はこのトレイトを実装して
/// メモリスラブの確保先を制御できる。
pub trait Allocator {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>>;

    /// 以前に確保したブロックを解放する。
    ///
    /// アリーナ実装では個別解放を無視し、スコープ終了時に一括回収してもよい。
    ///
    /// # Safety
    /// `ptr` は `self.alloc(layout)` が返した未解放のポインタでなければならない。
    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout);
}

/// Rust の標準グローバルヒープを使う既定のアロケータ。
#[derive(Debug, Default, Clone, Copy)]
pub struct HeapAllocator;

impl Allocator for HeapAllocator {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        if layout.size() == 0 {
            return Some(NonNull::dangling());
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

impl Allocator for ArenaAllocator<'_> {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        if layout.size() == 0 {
            return Some(NonNull::dangling());
        }
        let align = layout.align();
        let aligned = (self.offset + align - 1) & !(align - 1);
        let end = aligned.checked_add(layout.size())?;
        if end > self.buf.len() {
            return None;
        }
        self.offset = end;
        // SAFETY: `aligned < end <= self.buf.len()` が成り立つ。
        let ptr = unsafe { self.buf.as_mut_ptr().add(aligned) };
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

    pub fn as_slice(&self, len: usize) -> &[T] {
        debug_assert!(len <= self.cap);
        if len == 0 {
            return &[];
        }
        // SAFETY: `len <= self.cap` かつ `0..len` が初期化済み。
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), len) }
    }

    pub fn as_mut_slice(&mut self, len: usize) -> &mut [T] {
        debug_assert!(len <= self.cap);
        if len == 0 {
            return &mut [];
        }
        // SAFETY: `len <= self.cap` かつ `0..len` が初期化済み。
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), len) }
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
