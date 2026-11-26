//! 外部 `Allocator` 上の領域から確保する可変長配列 `RawVec` と作業領域ボックス `WorkBox`。

use std::alloc::Layout;
use std::marker::PhantomData;
use std::ptr::NonNull;

use super::{AllocCore, PermArena, WorkArena};

/// 作業領域 (`WorkArena`) 上に配置した単一値への軽量ポインタ（`Copy`・`Drop` 不要）。
pub(crate) struct WorkBox<T> {
    ptr: NonNull<T>,
}

impl<T> Clone for WorkBox<T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for WorkBox<T> {}

impl<T> WorkBox<T> {
    pub fn new(work: &mut WorkArena, core: &mut AllocCore<'_>, val: T) -> Option<Self> {
        let ptr = work.alloc_val(core, val)?;
        Some(Self { ptr })
    }

    pub fn try_new(work: &mut WorkArena, core: &mut AllocCore<'_>, val: T) -> Result<Self, T> {
        let layout = Layout::new::<T>();
        let Some(raw) = work.alloc_raw(core, layout) else {
            return Err(val);
        };
        let ptr = raw.cast::<T>();
        // SAFETY: `raw` は `T` のアラインメントとサイズで確保済み。
        unsafe { ptr.as_ptr().write(val) };
        Ok(Self { ptr })
    }

    #[inline]
    pub fn as_ref(&self) -> &T {
        // SAFETY: `WorkArena` が巻き戻されるまで有効。
        unsafe { self.ptr.as_ref() }
    }

    #[inline]
    pub fn as_mut(&mut self) -> &mut T {
        // SAFETY: `WorkArena` が巻き戻されるまで有効で排他参照。
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> std::ops::Deref for WorkBox<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for WorkBox<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_ref().fmt(f)
    }
}

/// 任意の領域でバッキングメモリを管理できる軽量可変長配列（`ptr`, `len`, `cap` の 3 ワード）。
///
/// 要素の解放やバッファの返却は、所有側の領域（`WorkArena` / `PermArena` / `ObjHeap`）に合わせて行う。
pub(crate) struct RawVec<T> {
    ptr: NonNull<T>,
    len: u32,
    cap: u32,
    _marker: PhantomData<T>,
}

impl<T> Clone for RawVec<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            ptr: self.ptr,
            len: self.len,
            cap: self.cap,
            _marker: PhantomData,
        }
    }
}

impl<T: Copy> Copy for RawVec<T> {}

impl<T: std::fmt::Debug> std::fmt::Debug for RawVec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_slice().fmt(f)
    }
}

impl<T> std::ops::Index<usize> for RawVec<T> {
    type Output = T;

    #[inline(always)]
    fn index(&self, index: usize) -> &Self::Output {
        assert!(index < self.len as usize);
        // SAFETY: 直上で `index < self.len` を検査済み。
        unsafe { &*self.ptr.as_ptr().add(index) }
    }
}

impl<T> std::ops::IndexMut<usize> for RawVec<T> {
    #[inline(always)]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        assert!(index < self.len as usize);
        // SAFETY: 直上で `index < self.len` を検査済み。
        unsafe { &mut *self.ptr.as_ptr().add(index) }
    }
}

impl<T> Default for RawVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> RawVec<T> {
    pub const fn new() -> Self {
        Self {
            ptr: NonNull::dangling(),
            len: 0,
            cap: 0,
            _marker: PhantomData,
        }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.cap as usize
    }

    #[inline(always)]
    pub fn get_copy(&self, index: usize) -> T
    where
        T: Copy,
    {
        assert!(index < self.len as usize);
        // SAFETY: 直上で `index < self.len` を検査済み。
        unsafe { *self.ptr.as_ptr().add(index) }
    }

    #[inline(always)]
    pub fn set_at(&mut self, index: usize, value: T) {
        assert!(index < self.len as usize);
        // SAFETY: 直上で `index < self.len` を検査済み。
        unsafe {
            *self.ptr.as_ptr().add(index) = value;
        }
    }

    #[inline(always)]
    pub fn replace_at(&mut self, index: usize, value: T) -> T {
        assert!(index < self.len as usize);
        // SAFETY: 直上で `index < self.len` を検査済み。
        unsafe { std::ptr::replace(self.ptr.as_ptr().add(index), value) }
    }

    #[inline(always)]
    pub fn as_slice(&self) -> &[T] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: `0..self.len` は初期化済みで有効。
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len as usize) }
    }

    #[inline(always)]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.len == 0 {
            return &mut [];
        }
        // SAFETY: `0..self.len` は初期化済みで排他参照。
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len as usize) }
    }

    #[inline(always)]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        // SAFETY: `self.len` 位置は初期化済み。
        Some(unsafe { self.ptr.as_ptr().add(self.len as usize).read() })
    }

    #[inline(always)]
    pub fn last(&self) -> Option<&T> {
        self.as_slice().last()
    }

    #[inline(always)]
    pub fn last_mut(&mut self) -> Option<&mut T> {
        self.as_mut_slice().last_mut()
    }

    #[inline]
    pub fn clear(&mut self) {
        if std::mem::needs_drop::<T>() {
            for i in 0..self.len as usize {
                // SAFETY: `0..self.len` は初期化済み。
                unsafe { std::ptr::drop_in_place(self.ptr.as_ptr().add(i)) };
            }
        }
        self.len = 0;
    }

    #[inline(always)]
    pub fn truncate(&mut self, new_len: usize) {
        if new_len >= self.len as usize {
            return;
        }
        if std::mem::needs_drop::<T>() {
            for i in new_len..self.len as usize {
                // SAFETY: `new_len..self.len` は初期化済み。
                unsafe { std::ptr::drop_in_place(self.ptr.as_ptr().add(i)) };
            }
        }
        self.len = new_len as u32;
    }

    fn next_cap(cur: usize, additional: usize) -> Option<usize> {
        let required = cur.checked_add(additional)?;
        if required > u32::MAX as usize {
            return None;
        }
        let doubled = cur.saturating_mul(2).max(2).min(u32::MAX as usize);
        Some(doubled.max(required))
    }

    /// 作業領域上に領域を拡張して末尾へ要素を追加する。
    #[inline(always)]
    pub fn push_work(
        &mut self,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        value: T,
    ) -> Option<()> {
        if self.len == self.cap {
            self.reserve_work_slow(work, core, 1)?;
        }
        // SAFETY: `self.len < self.cap`。
        unsafe { self.ptr.as_ptr().add(self.len as usize).write(value) };
        self.len += 1;
        Some(())
    }

    #[inline(always)]
    pub fn reserve_work(
        &mut self,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        additional: usize,
    ) -> Option<()> {
        if (self.cap as usize).saturating_sub(self.len as usize) >= additional {
            return Some(());
        }
        self.reserve_work_slow(work, core, additional)
    }

    #[cold]
    #[inline(never)]
    fn reserve_work_slow(
        &mut self,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        additional: usize,
    ) -> Option<()> {
        let new_cap = Self::next_cap(self.len as usize, additional)?;
        if std::mem::size_of::<T>() == 0 {
            self.cap = new_cap as u32;
            return Some(());
        }
        let layout = Layout::array::<T>(new_cap).ok()?;
        if self.cap > 0 {
            let old_size = (self.cap as usize) * std::mem::size_of::<T>();
            // SAFETY: `self.ptr` はこの `WorkArena` から `old_size` バイト確保された領域。
            if unsafe {
                work.try_grow_in_place(core, self.ptr.cast::<u8>(), old_size, layout.size())
            } {
                self.cap = new_cap as u32;
                return Some(());
            }
        }
        let raw = work.alloc_raw(core, layout)?;
        let new_ptr = raw.cast::<T>();
        if self.len > 0 {
            // SAFETY: 旧領域と新領域は非重複で `self.len` 要素分有効。
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.ptr.as_ptr(),
                    new_ptr.as_ptr(),
                    self.len as usize,
                )
            };
        }
        self.ptr = new_ptr;
        self.cap = new_cap as u32;
        Some(())
    }

    /// 永続領域上に領域を拡張して末尾へ要素を追加する。
    pub fn push_perm(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        value: T,
    ) -> Option<()> {
        if self.len == self.cap {
            self.reserve_perm(perm, core, 1)?;
        }
        // SAFETY: `self.len < self.cap`。
        unsafe { self.ptr.as_ptr().add(self.len as usize).write(value) };
        self.len += 1;
        Some(())
    }

    pub fn reserve_perm(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        additional: usize,
    ) -> Option<()> {
        if (self.cap as usize).saturating_sub(self.len as usize) >= additional {
            return Some(());
        }
        let new_cap = Self::next_cap(self.len as usize, additional)?;
        if std::mem::size_of::<T>() == 0 {
            self.cap = new_cap as u32;
            return Some(());
        }
        let layout = Layout::array::<T>(new_cap).ok()?;
        if self.cap > 0 {
            let old_size = (self.cap as usize) * std::mem::size_of::<T>();
            // SAFETY: `self.ptr` はこの `PermArena` から `old_size` バイト確保された領域。
            if unsafe {
                perm.try_grow_in_place(core, self.ptr.cast::<u8>(), old_size, layout.size())
            } {
                self.cap = new_cap as u32;
                return Some(());
            }
        }
        let raw = perm.alloc_raw(core, layout)?;
        let new_ptr = raw.cast::<T>();
        if self.len > 0 {
            // SAFETY: 旧領域と新領域は非重複で `self.len` 要素分有効。
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.ptr.as_ptr(),
                    new_ptr.as_ptr(),
                    self.len as usize,
                )
            };
        }
        self.ptr = new_ptr;
        self.cap = new_cap as u32;
        Some(())
    }
}

impl<T: Copy> RawVec<T> {
    #[inline(always)]
    pub fn resize_work(
        &mut self,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        new_len: usize,
        fill: T,
    ) -> Option<()> {
        if new_len <= self.len as usize {
            self.len = new_len as u32;
            return Some(());
        }
        self.reserve_work(work, core, new_len - self.len as usize)?;
        for i in self.len as usize..new_len {
            // SAFETY: `i < self.cap`。
            unsafe { self.ptr.as_ptr().add(i).write(fill) };
        }
        self.len = new_len as u32;
        Some(())
    }

    pub fn resize_perm(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        new_len: usize,
        fill: T,
    ) -> Option<()> {
        if new_len <= self.len as usize {
            self.len = new_len as u32;
            return Some(());
        }
        self.reserve_perm(perm, core, new_len - self.len as usize)?;
        for i in self.len as usize..new_len {
            // SAFETY: `i < self.cap`。
            unsafe { self.ptr.as_ptr().add(i).write(fill) };
        }
        self.len = new_len as u32;
        Some(())
    }

    pub fn extend_from_slice_work(
        &mut self,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        src: &[T],
    ) -> Option<()> {
        if src.is_empty() {
            return Some(());
        }
        self.reserve_work(work, core, src.len())?;
        // SAFETY: `self.len + src.len() <= self.cap` かつ非重複。
        unsafe {
            std::ptr::copy_nonoverlapping(
                src.as_ptr(),
                self.ptr.as_ptr().add(self.len as usize),
                src.len(),
            );
        }
        self.len += src.len() as u32;
        Some(())
    }

    pub fn extend_from_slice_perm(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        src: &[T],
    ) -> Option<()> {
        if src.is_empty() {
            return Some(());
        }
        self.reserve_perm(perm, core, src.len())?;
        // SAFETY: `self.len + src.len() <= self.cap` かつ非重複。
        unsafe {
            std::ptr::copy_nonoverlapping(
                src.as_ptr(),
                self.ptr.as_ptr().add(self.len as usize),
                src.len(),
            );
        }
        self.len += src.len() as u32;
        Some(())
    }

    /// ちょうど `src.len()` 要素分の領域を永続領域へ確保してコピーする（余剰容量ゼロ）。
    pub fn from_slice_exact_perm(
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        src: &[T],
    ) -> Option<Self> {
        if src.is_empty() {
            return Some(Self::new());
        }
        let len = u32::try_from(src.len()).ok()?;
        let ptr = perm.alloc_copy_slice(core, src)?;
        Some(Self {
            ptr,
            len,
            cap: len,
            _marker: PhantomData,
        })
    }
}
