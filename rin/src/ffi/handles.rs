//! C FFI のアロケータ橋渡し・値ハンドルプール・内部マシン状態。

use std::alloc::Layout;
use std::ptr::{NonNull, addr_of_mut};

use crate::alloc::{Allocator, HeapAllocator};
use crate::eval::{Builder, Machine};
use crate::value_ref::{Val, ValueRef};

use super::RinAllocator;

#[derive(Clone, Copy)]
pub(super) struct CAllocator {
    pub(super) raw: RinAllocator,
}

impl Allocator for CAllocator {
    fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        if let Some(f) = self.raw.alloc {
            // SAFETY: 呼び出し側がマシンの生存期間中 `f` と `self.raw.ctx` の有効性を保証する。
            let ptr = unsafe { f(self.raw.ctx, layout.size(), layout.align()) };
            NonNull::new(ptr)
        } else {
            HeapAllocator.alloc(layout)
        }
    }

    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, layout: Layout) {
        if let Some(f) = self.raw.dealloc {
            // SAFETY: 呼び出し側が `alloc` に対応する `f` であることを保証する。
            unsafe { f(self.raw.ctx, ptr.as_ptr(), layout.size(), layout.align()) };
        } else if self.raw.alloc.is_none() {
            // SAFETY: `self.raw.alloc` が `None` のため `HeapAllocator` で確保した領域。
            unsafe { HeapAllocator.dealloc(ptr, layout) };
        }
    }
}

/// マシン所有の値ハンドル。`rin_int` 等で取得し `rin_drop` で解放する。
#[repr(transparent)]
pub struct RinValue(pub(super) Val);

const INLINE_HANDLES: usize = 32;
const CHUNK_HANDLES: usize = 64;

#[repr(C)]
pub(super) struct HandleSlot {
    pub(super) val: RinValue,
    next_free: *mut HandleSlot,
    owner: *const ValueHandles,
    alive: bool,
}

impl HandleSlot {
    pub(super) const fn empty() -> Self {
        Self {
            val: RinValue(Val::void()),
            next_free: std::ptr::null_mut(),
            owner: std::ptr::null(),
            alive: false,
        }
    }
}

struct HandleChunk {
    next: *mut HandleChunk,
    used: usize,
    slots: [HandleSlot; CHUNK_HANDLES],
}

/// `RinValue` ハンドルの安定ポインタを `CAllocator` のみで管理するプール。
pub(super) struct ValueHandles {
    inline: [HandleSlot; INLINE_HANDLES],
    inline_used: usize,
    chunks_head: *mut HandleChunk,
    free_head: *mut HandleSlot,
}

impl ValueHandles {
    pub(super) const fn new() -> Self {
        Self {
            inline: [const { HandleSlot::empty() }; INLINE_HANDLES],
            inline_used: 0,
            chunks_head: std::ptr::null_mut(),
            free_head: std::ptr::null_mut(),
        }
    }

    #[inline]
    pub(super) fn alloc_slot(&mut self, val: Val, alloc: &mut CAllocator) -> Option<*mut RinValue> {
        let owner = self as *const ValueHandles;
        if !self.free_head.is_null() {
            let slot = self.free_head;
            // SAFETY: `free_head` は当プール所有の有効な空きスロットを指す。
            unsafe {
                self.free_head = (*slot).next_free;
                (*slot).val = RinValue(val);
                (*slot).next_free = std::ptr::null_mut();
                (*slot).owner = owner;
                (*slot).alive = true;
                return Some(addr_of_mut!((*slot).val));
            }
        }
        if self.inline_used < INLINE_HANDLES {
            let slot = &mut self.inline[self.inline_used];
            self.inline_used += 1;
            slot.val = RinValue(val);
            slot.next_free = std::ptr::null_mut();
            slot.owner = owner;
            slot.alive = true;
            return Some(addr_of_mut!(slot.val));
        }
        // SAFETY: `chunks_head` は NULL または `alloc` から確保した有効な `HandleChunk`。
        let need_new =
            self.chunks_head.is_null() || unsafe { (*self.chunks_head).used } == CHUNK_HANDLES;
        if need_new {
            let layout = Layout::new::<HandleChunk>();
            let raw = alloc.alloc(layout)?;
            let chunk = raw.cast::<HandleChunk>().as_ptr();
            // SAFETY: `raw` は `HandleChunk` のレイアウトで確保済み。
            unsafe {
                addr_of_mut!((*chunk).next).write(self.chunks_head);
                addr_of_mut!((*chunk).used).write(0);
                let slots_ptr = addr_of_mut!((*chunk).slots).cast::<HandleSlot>();
                for i in 0..CHUNK_HANDLES {
                    slots_ptr.add(i).write(HandleSlot::empty());
                }
            }
            self.chunks_head = chunk;
        }
        // SAFETY: `chunks_head` は非 NULL かつ `used < CHUNK_HANDLES`。
        unsafe {
            let chunk = &mut *self.chunks_head;
            let slot = &mut chunk.slots[chunk.used];
            chunk.used += 1;
            slot.val = RinValue(val);
            slot.next_free = std::ptr::null_mut();
            slot.owner = owner;
            slot.alive = true;
            Some(addr_of_mut!(slot.val))
        }
    }

    /// `val_ptr` が当プール所有の生存中ハンドルかどうかを O(1) で判定する。
    ///
    /// # Safety
    /// `val_ptr` は `NULL`、`alloc_slot` が返したハンドル、または `Rin.last_result.val`（`HandleSlot` 先頭）
    /// のいずれかへの有効なポインタでなければならない。
    pub(super) unsafe fn owns_live_val(&self, val_ptr: *const RinValue) -> bool {
        if val_ptr.is_null() {
            return false;
        }
        if !(val_ptr as usize).is_multiple_of(std::mem::align_of::<HandleSlot>()) {
            return false;
        }
        let slot = val_ptr.cast::<HandleSlot>();
        // SAFETY: 呼び出し側が有効な `HandleSlot` 先頭ポインタを保証する。
        unsafe { (*slot).alive && std::ptr::eq((*slot).owner, self) }
    }

    /// `val_ptr` を解放リストへ O(1) で返し、保持していた `Val` を返す。
    /// 当プールに属さないポインタ（別の `Rin` のハンドルや `RinEvalResult.value` 等）は安全のため無視して `None` を返す。
    #[inline]
    pub(super) unsafe fn free_slot(&mut self, val_ptr: *mut RinValue) -> Option<Val> {
        if !unsafe { self.owns_live_val(val_ptr) } {
            return None;
        }
        let slot = val_ptr.cast::<HandleSlot>();
        // SAFETY: `owns_live_val` により `slot` は当プール所有の生存中 `HandleSlot`。
        unsafe {
            let val = (*slot).val.0;
            (*slot).alive = false;
            (*slot).owner = std::ptr::null();
            (*slot).val = RinValue(Val::void());
            (*slot).next_free = self.free_head;
            self.free_head = slot;
            Some(val)
        }
    }

    pub(super) fn release_all_and_destroy(&mut self, state: &mut RinState, alloc: &mut CAllocator) {
        for slot in &mut self.inline[..self.inline_used] {
            if slot.alive {
                slot.alive = false;
                state.release_val(slot.val.0);
            }
        }
        let mut cur = self.chunks_head;
        self.chunks_head = std::ptr::null_mut();
        self.free_head = std::ptr::null_mut();
        let layout = Layout::new::<HandleChunk>();
        while !cur.is_null() {
            // SAFETY: `cur` は `alloc.alloc(Layout::new::<HandleChunk>())` で確保したチャンク。
            unsafe {
                let next = (*cur).next;
                let used = (*cur).used;
                let slots_ptr = addr_of_mut!((*cur).slots).cast::<HandleSlot>();
                for i in 0..used {
                    let slot = &mut *slots_ptr.add(i);
                    if slot.alive {
                        slot.alive = false;
                        state.release_val(slot.val.0);
                    }
                }
                alloc.dealloc(NonNull::new_unchecked(cur.cast::<u8>()), layout);
                cur = next;
            }
        }
    }
}

pub(super) enum RinState {
    Building(Builder<'static>),
    Frozen(Machine<'static>),
}

impl RinState {
    #[inline]
    pub(super) fn release_val(&mut self, v: Val) {
        match self {
            RinState::Building(b) => b.release_val(v),
            RinState::Frozen(f) => f.release_val(v),
        }
    }

    pub(super) fn retain_val(&mut self, v: Val) {
        match self {
            RinState::Building(b) => b.retain_val(v),
            RinState::Frozen(f) => f.retain_val(v),
        }
    }

    pub(super) fn value_ref(&self, v: Val) -> ValueRef<'_> {
        match self {
            RinState::Building(b) => b.value_ref(v),
            RinState::Frozen(f) => f.value_ref(v),
        }
    }

    pub(super) fn last_result_val(&self) -> Val {
        match self {
            RinState::Building(b) => b.last_result_val(),
            RinState::Frozen(f) => f.last_result_val(),
        }
    }
}
