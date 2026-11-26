//! 外部 `Allocator` 上の 3 領域から確保する軽量な自前コンテナ（`RawVec`・`RawStr`・`RawMap`）。

use std::alloc::Layout;
use std::marker::PhantomData;
use std::ptr::NonNull;

use super::{AllocCore, ObjBlock, ObjHeap, PermArena, WorkArena};

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
        &self.as_slice()[index]
    }
}

impl<T> std::ops::IndexMut<usize> for RawVec<T> {
    #[inline(always)]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.as_mut_slice()[index]
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

/// 領域上に配置した UTF-8 文字列ビュー（2 ワード）。
#[derive(Clone, Copy)]
pub(crate) struct RawStr {
    ptr: NonNull<u8>,
    len: usize,
}

impl RawStr {
    pub const fn empty() -> Self {
        Self {
            ptr: NonNull::dangling(),
            len: 0,
        }
    }

    /// 既に有効な UTF-8 バイトスライス（`WorkArena` や入力 `&str` 上）から構築する。
    ///
    /// # Safety
    /// `bytes` がこの `RawStr` の使用期間中ずっと有効な UTF-8 でなければならない。
    pub unsafe fn from_raw_utf8(ptr: NonNull<u8>, len: usize) -> Self {
        Self { ptr, len }
    }

    pub fn from_perm(perm: &mut PermArena, core: &mut AllocCore<'_>, s: &str) -> Option<Self> {
        if s.is_empty() {
            return Some(Self::empty());
        }
        let ptr = perm.alloc_copy_slice(core, s.as_bytes())?;
        Some(Self { ptr, len: s.len() })
    }

    /// NUL 終端バイト付きで永続領域へ確保する（C FFI 向け）。
    pub fn from_perm_cstr(perm: &mut PermArena, core: &mut AllocCore<'_>, s: &str) -> Option<Self> {
        let total = s.len().checked_add(1)?;
        let layout = Layout::from_size_align(total, 1).ok()?;
        let ptr = perm.alloc_raw(core, layout)?;
        // SAFETY: `total == s.len() + 1` バイト確保済み。
        unsafe {
            if !s.is_empty() {
                std::ptr::copy_nonoverlapping(s.as_ptr(), ptr.as_ptr(), s.len());
            }
            ptr.as_ptr().add(s.len()).write(0);
        }
        Some(Self { ptr, len: s.len() })
    }

    pub fn from_work(work: &mut WorkArena, core: &mut AllocCore<'_>, s: &str) -> Option<Self> {
        if s.is_empty() {
            return Some(Self::empty());
        }
        let layout = Layout::from_size_align(s.len(), 1).ok()?;
        let ptr = work.alloc_raw(core, layout)?;
        // SAFETY: `s.len()` バイト確保済み。
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), ptr.as_ptr(), s.len()) };
        Some(Self { ptr, len: s.len() })
    }

    /// NUL 終端バイト付きで作業領域へ確保する（C FFI 向け）。
    pub fn from_work_cstr(work: &mut WorkArena, core: &mut AllocCore<'_>, s: &str) -> Option<Self> {
        let total = s.len().checked_add(1)?;
        let layout = Layout::from_size_align(total, 1).ok()?;
        let ptr = work.alloc_raw(core, layout)?;
        // SAFETY: `total == s.len() + 1` バイト確保済み。
        unsafe {
            if !s.is_empty() {
                std::ptr::copy_nonoverlapping(s.as_ptr(), ptr.as_ptr(), s.len());
            }
            ptr.as_ptr().add(s.len()).write(0);
        }
        Some(Self { ptr, len: s.len() })
    }

    /// オブジェクト領域から確保して文字列とブロック情報を返す。
    pub fn from_obj(
        heap: &mut ObjHeap,
        core: &mut AllocCore<'_>,
        s: &str,
    ) -> Option<(Self, ObjBlock)> {
        if s.is_empty() {
            return Some((
                Self::empty(),
                ObjBlock {
                    ptr: NonNull::dangling(),
                    actual_size: 0,
                },
            ));
        }
        let block = heap.alloc(core, s.len(), 1)?;
        // SAFETY: `block.actual_size >= s.len()`。
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), block.ptr.as_ptr(), s.len()) };
        Some((
            Self {
                ptr: block.ptr,
                len: s.len(),
            },
            block,
        ))
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        if self.len == 0 {
            return "";
        }
        // SAFETY: 構築時に有効な `&str` のバイト列をコピーしている。
        unsafe {
            let bytes = std::slice::from_raw_parts(self.ptr.as_ptr(), self.len);
            std::str::from_utf8_unchecked(bytes)
        }
    }

    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl PartialEq for RawStr {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for RawStr {}

impl AsRef<str> for RawStr {
    #[inline(always)]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::ops::Deref for RawStr {
    type Target = str;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl std::fmt::Display for RawStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Debug for RawStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}

/// FNV-1a 64bit ハッシュ。
#[inline]
pub(crate) fn fnv1a_bytes(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[inline]
pub(crate) fn fnv1a_u32(val: u32) -> u64 {
    fnv1a_bytes(&val.to_le_bytes())
}

#[derive(Clone, Copy)]
struct MapSlot<K: Copy, V: Copy> {
    occupied: bool,
    hash: u64,
    key: K,
    value: V,
}

/// オープンアドレス線形探査ハッシュ表（`PermArena` または `WorkArena` 上で動作）。
pub(crate) struct RawMap<K: Copy, V: Copy> {
    slots: NonNull<MapSlot<K, V>>,
    cap: usize,
    len: usize,
}

impl<K: Copy, V: Copy> Default for RawMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Copy, V: Copy> RawMap<K, V> {
    pub const fn new() -> Self {
        Self {
            slots: NonNull::dangling(),
            cap: 0,
            len: 0,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    fn slots_slice(&self) -> &[MapSlot<K, V>] {
        if self.cap == 0 {
            return &[];
        }
        // SAFETY: `self.cap` 個の `MapSlot` が初期化済み。
        unsafe { std::slice::from_raw_parts(self.slots.as_ptr(), self.cap) }
    }

    #[inline]
    fn slots_mut(&mut self) -> &mut [MapSlot<K, V>] {
        if self.cap == 0 {
            return &mut [];
        }
        // SAFETY: `self.cap` 個の `MapSlot` が初期化済み。
        unsafe { std::slice::from_raw_parts_mut(self.slots.as_ptr(), self.cap) }
    }

    pub fn get_by(&self, hash: u64, mut eq: impl FnMut(&K) -> bool) -> Option<V> {
        if self.cap == 0 {
            return None;
        }
        let mask = self.cap - 1;
        let slots = self.slots_slice();
        let mut idx = (hash as usize) & mask;
        loop {
            let slot = &slots[idx];
            if !slot.occupied {
                return None;
            }
            if slot.hash == hash && eq(&slot.key) {
                return Some(slot.value);
            }
            idx = (idx + 1) & mask;
        }
    }

    pub fn insert_perm(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        hash: u64,
        key: K,
        value: V,
        mut eq: impl FnMut(&K, &K) -> bool,
    ) -> Option<Option<V>> {
        if self.len * 10 >= self.cap * 7 {
            let new_cap = (self.cap * 2).max(16);
            self.rehash_with(new_cap, |layout| perm.alloc_raw(core, layout))?;
        }
        Some(self.insert_no_grow(hash, key, value, &mut eq))
    }

    pub fn insert_work(
        &mut self,
        work: &mut WorkArena,
        core: &mut AllocCore<'_>,
        hash: u64,
        key: K,
        value: V,
        mut eq: impl FnMut(&K, &K) -> bool,
    ) -> Option<Option<V>> {
        if self.len * 10 >= self.cap * 7 {
            let new_cap = (self.cap * 2).max(16);
            self.rehash_with(new_cap, |layout| work.alloc_raw(core, layout))?;
        }
        Some(self.insert_no_grow(hash, key, value, &mut eq))
    }

    fn insert_no_grow(
        &mut self,
        hash: u64,
        key: K,
        value: V,
        eq: &mut impl FnMut(&K, &K) -> bool,
    ) -> Option<V> {
        let mask = self.cap - 1;
        let slots = self.slots_mut();
        let mut idx = (hash as usize) & mask;
        loop {
            let slot = &mut slots[idx];
            if !slot.occupied {
                slot.occupied = true;
                slot.hash = hash;
                slot.key = key;
                slot.value = value;
                self.len += 1;
                return None;
            }
            if slot.hash == hash && eq(&slot.key, &key) {
                let prev = slot.value;
                slot.value = value;
                return Some(prev);
            }
            idx = (idx + 1) & mask;
        }
    }

    fn rehash_with(
        &mut self,
        new_cap: usize,
        alloc_fn: impl FnOnce(Layout) -> Option<NonNull<u8>>,
    ) -> Option<()> {
        debug_assert!(new_cap.is_power_of_two());
        let layout = Layout::array::<MapSlot<K, V>>(new_cap).ok()?;
        let raw = alloc_fn(layout)?;
        let new_slots = raw.cast::<MapSlot<K, V>>();
        // SAFETY: `new_cap` 個分の領域を確保済み。`occupied = false` でゼロ初期化相当。
        unsafe {
            std::ptr::write_bytes(new_slots.as_ptr(), 0, new_cap);
        }
        let mask = new_cap - 1;
        // SAFETY: 新規領域は `new_cap` 個分初期化済み。
        let dst = unsafe { std::slice::from_raw_parts_mut(new_slots.as_ptr(), new_cap) };
        for slot in self.slots_slice() {
            if slot.occupied {
                let mut idx = (slot.hash as usize) & mask;
                while dst[idx].occupied {
                    idx = (idx + 1) & mask;
                }
                dst[idx] = *slot;
            }
        }
        self.slots = new_slots;
        self.cap = new_cap;
        Some(())
    }

    pub fn iter(&self) -> impl Iterator<Item = (K, V)> + '_ {
        self.slots_slice()
            .iter()
            .filter(|s| s.occupied)
            .map(|s| (s.key, s.value))
    }
}
