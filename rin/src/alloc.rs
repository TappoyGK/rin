use std::alloc::Layout;
use std::marker::PhantomData;
use std::ptr::NonNull;

/// External memory allocator interface for Rin's runtime pool.
///
/// Hosts (such as an arena-backed runtime) can implement this trait to control
/// where Rin allocates its backing memory slabs.
pub trait Allocator {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>>;

    /// Releases a previously allocated block.
    ///
    /// Arena implementations may treat this as a no-op and reclaim all memory
    /// when the arena scope ends.
    ///
    /// # Safety
    /// `ptr` must have been returned by `self.alloc(layout)` and not yet deallocated.
    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout);
}

/// Default allocator backed by Rust's global heap allocator.
#[derive(Debug, Default, Clone, Copy)]
pub struct HeapAllocator;

impl Allocator for HeapAllocator {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        if layout.size() == 0 {
            return Some(NonNull::dangling());
        }
        // SAFETY: `layout.size() > 0` is checked above.
        let ptr = unsafe { std::alloc::alloc(layout) };
        NonNull::new(ptr)
    }

    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() == 0 {
            return;
        }
        // SAFETY: Caller guarantees `ptr` came from `std::alloc::alloc(layout)`.
        unsafe { std::alloc::dealloc(ptr.as_ptr(), layout) };
    }
}

/// Bump-pointer arena allocator over a caller-provided byte slice.
///
/// Individual `dealloc` calls are no-ops; all memory is reclaimed when the
/// underlying slice is dropped or reset.
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
        // SAFETY: `aligned < end <= self.buf.len()`.
        let ptr = unsafe { self.buf.as_mut_ptr().add(aligned) };
        NonNull::new(ptr)
    }

    unsafe fn dealloc(&mut self, _ptr: NonNull<u8>, _layout: Layout) {
        // Arena reclaims memory in bulk when the backing buffer is dropped/reset.
    }
}

/// Contiguous buffer of `T` allocated from an `Allocator`.
pub(crate) struct AllocBuf<T> {
    ptr: NonNull<T>,
    cap: usize,
    _marker: PhantomData<T>,
}

impl<T> AllocBuf<T> {
    pub fn empty() -> Self {
        Self {
            ptr: NonNull::dangling(),
            cap: 0,
            _marker: PhantomData,
        }
    }

    pub fn with_capacity(alloc: &mut dyn Allocator, cap: usize) -> Option<Self> {
        if cap == 0 || std::mem::size_of::<T>() == 0 {
            return Some(Self::empty());
        }
        let layout = Layout::array::<T>(cap).ok()?;
        let raw = alloc.alloc(layout)?;
        Some(Self {
            ptr: raw.cast::<T>(),
            cap,
            _marker: PhantomData,
        })
    }

    pub fn as_slice(&self, len: usize) -> &[T] {
        debug_assert!(len <= self.cap);
        if len == 0 {
            return &[];
        }
        // SAFETY: `len <= self.cap` and elements `0..len` are initialized.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), len) }
    }

    pub fn as_mut_slice(&mut self, len: usize) -> &mut [T] {
        debug_assert!(len <= self.cap);
        if len == 0 {
            return &mut [];
        }
        // SAFETY: `len <= self.cap` and elements `0..len` are initialized.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), len) }
    }

    /// Writes `value` at uninitialized index `idx`.
    ///
    /// # Safety
    /// `idx < self.cap` and slot `idx` must currently be uninitialized.
    pub unsafe fn write_at(&mut self, idx: usize, value: T) {
        debug_assert!(idx < self.cap);
        // SAFETY: Caller guarantees `idx < self.cap` and slot is uninitialized.
        unsafe { self.ptr.as_ptr().add(idx).write(value) };
    }

    /// Releases the backing allocation via `alloc` after dropping `0..len` initialized items.
    pub fn destroy(&mut self, alloc: &mut dyn Allocator, len: usize) {
        debug_assert!(len <= self.cap);
        if self.cap == 0 || std::mem::size_of::<T>() == 0 {
            return;
        }
        for i in 0..len {
            // SAFETY: `0..len` are initialized.
            unsafe { std::ptr::drop_in_place(self.ptr.as_ptr().add(i)) };
        }
        if let Ok(layout) = Layout::array::<T>(self.cap) {
            // SAFETY: `self.ptr` was allocated with `layout` from `alloc`.
            unsafe { alloc.dealloc(self.ptr.cast::<u8>(), layout) };
        }
        self.ptr = NonNull::dangling();
        self.cap = 0;
    }
}
