//! 外部 `Allocator` 上の領域で動作するオープンアドレスハッシュ表 `RawMap` と FNV-1a ハッシュ関数。

use std::alloc::Layout;
use std::ptr::NonNull;

use super::{AllocCore, PermArena, WorkArena};

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
