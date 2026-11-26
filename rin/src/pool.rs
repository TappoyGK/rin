//! 外部アロケータ上で動作する参照カウント付きオブジェクトプールと 3 領域メモリ管理を実装する。

use std::alloc::Layout;
use std::ptr::NonNull;

use crate::alloc::{AllocBuf, Allocator};
#[cfg(feature = "std-value")]
use crate::bytecode::BuiltinId;
use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::mem::{AllocCore, MemoryConfig, ObjHeap, PermArena, WorkArena};
#[cfg(feature = "std-value")]
use crate::value::Value;

mod alloc;
#[cfg(feature = "std-value")]
mod convert;
#[cfg(all(test, feature = "std-value"))]
mod release_tests;
#[cfg(feature = "std-value")]
mod shallow;
mod struct_desc;
mod types;

pub(crate) use types::{
    CodeRegion, ModuleExport, Obj, ObjId, ObjSlice, ObjStr, RVal, StructDescRef,
};

use struct_desc::StructDescTable;

const CHUNK_SLOTS: usize = 256;

enum Slot {
    Free { next: Option<u32> },
    Live { rc: u32, obj: Obj },
    Dying { next: Option<u32>, obj: Obj },
}

const SLOT_BYTES: usize = std::mem::size_of::<Slot>();

/// `Slot` チャンク配列のヘッダ（`Allocator` から直接確保し、`std::vec::Vec` を使わない）。
struct ChunkTable {
    ptr: NonNull<AllocBuf<Slot>>,
    len: usize,
    cap: usize,
}

impl ChunkTable {
    const fn new() -> Self {
        Self {
            ptr: NonNull::dangling(),
            len: 0,
            cap: 0,
        }
    }

    #[inline]
    fn len(&self) -> usize {
        self.len
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    fn as_slice(&self) -> &[AllocBuf<Slot>] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: `0..self.len` は初期化済み。
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    #[inline(always)]
    fn chunk_slot_ptr(&self, chunk_idx: usize) -> *mut Slot {
        debug_assert!(chunk_idx < self.len);
        // SAFETY: `chunk_idx < self.len` は初期化済みの `AllocBuf<Slot>`。
        unsafe { (*self.ptr.as_ptr().add(chunk_idx)).as_ptr() as *mut Slot }
    }

    #[inline(always)]
    fn write_slot(&mut self, chunk_idx: usize, in_chunk: usize, slot: Slot) {
        debug_assert!(chunk_idx < self.len);
        // SAFETY: `chunk_idx < self.len` かつ `in_chunk < CHUNK_SLOTS`。
        unsafe {
            (*self.ptr.as_ptr().add(chunk_idx)).write_at(in_chunk, slot);
        }
    }

    fn push(&mut self, core: &mut AllocCore<'_>, buf: AllocBuf<Slot>) -> Option<()> {
        if self.len == self.cap {
            let new_cap = (self.cap * 2).max(4);
            let new_layout = Layout::array::<AllocBuf<Slot>>(new_cap).ok()?;
            let new_raw = core.alloc_raw(new_layout)?;
            let new_ptr = new_raw.cast::<AllocBuf<Slot>>();
            if self.len > 0 {
                // SAFETY: 旧領域から `self.len` 要素を移動コピーする。
                unsafe {
                    std::ptr::copy_nonoverlapping(self.ptr.as_ptr(), new_ptr.as_ptr(), self.len);
                    if let Ok(old_layout) = Layout::array::<AllocBuf<Slot>>(self.cap) {
                        core.dealloc_raw(self.ptr.cast::<u8>(), old_layout);
                    }
                }
            }
            self.ptr = new_ptr;
            self.cap = new_cap;
        }
        // SAFETY: `self.len < self.cap`。
        unsafe { self.ptr.as_ptr().add(self.len).write(buf) };
        self.len += 1;
        Some(())
    }

    fn destroy(&mut self, core: &mut AllocCore<'_>, _last_chunk_len: usize) {
        for idx in 0..self.len {
            // SAFETY: `idx < self.len` は初期化済み。
            let chunk = unsafe { self.ptr.as_ptr().add(idx).read() };
            let chunk_cap = chunk.capacity();
            let chunk_layout = Layout::array::<Slot>(chunk_cap).ok();
            if let Some(layout) = chunk_layout
                && chunk_cap > 0
            {
                let raw = chunk.into_raw_ptr().cast::<u8>();
                // SAFETY: `chunk` は `core.alloc_raw(layout)` 由来。
                unsafe { core.dealloc_raw(raw, layout) };
            }
        }
        if self.cap > 0
            && let Ok(layout) = Layout::array::<AllocBuf<Slot>>(self.cap)
        {
            // SAFETY: `self.ptr` は `core.alloc_raw(layout)` 由来。
            unsafe { core.dealloc_raw(self.ptr.cast::<u8>(), layout) };
        }
        self.ptr = NonNull::dangling();
        self.len = 0;
        self.cap = 0;
    }
}

/// 呼び出し側の `Allocator` 上で 3 領域メモリ管理と参照カウント付きオブジェクトプールを提供する。
///
/// 不要になったスロットとペイロードはフリーリストへ返し、次回確保時に即座に再利用する。
pub(crate) struct Pool<'a> {
    pub(crate) core: AllocCore<'a>,
    pub(crate) perm: PermArena,
    pub(crate) obj_heap: ObjHeap,
    pub(crate) work: WorkArena,
    struct_descs: StructDescTable,
    chunks: ChunkTable,
    /// 最終チャンク内の初期化済みスロット数。
    last_chunk_len: usize,
    free_head: Option<u32>,
    #[cfg(all(test, feature = "std-value"))]
    release_counts: release_tests::Counts,
}

impl<'a> Pool<'a> {
    #[cfg(all(test, feature = "std-value"))]
    pub fn new(alloc: &'a mut dyn Allocator, max_bytes: usize) -> Self {
        Self::with_config(alloc, max_bytes, MemoryConfig::default())
    }

    pub fn with_config(
        alloc: &'a mut dyn Allocator,
        max_bytes: usize,
        config: MemoryConfig,
    ) -> Self {
        Self {
            core: AllocCore::new(alloc, max_bytes),
            perm: PermArena::new(config.perm),
            obj_heap: ObjHeap::new(config.obj),
            work: WorkArena::new(config.work),
            struct_descs: StructDescTable::new(),
            chunks: ChunkTable::new(),
            last_chunk_len: 0,
            free_head: None,
            #[cfg(all(test, feature = "std-value"))]
            release_counts: release_tests::Counts::default(),
        }
    }

    /// 永続領域を凍結し、以降の書き込みを禁止する。
    pub fn freeze_perm(&mut self) {
        self.perm.freeze();
    }

    /// 現在オブジェクト領域上で生存している実確保バイト数を返す。
    #[inline]
    pub fn live_bytes(&self) -> usize {
        self.core.obj_bytes()
    }

    #[cold]
    fn oom_error(&mut self, span: Span, msg: &str) -> ErrRecorded {
        self.core
            .err_str("eval.limits", ErrorKind::TooMuchMemory, span, msg)
    }

    #[cold]
    fn oom_limit_error(&mut self, span: Span) -> ErrRecorded {
        let max = self.core.max_bytes();
        self.core.err_fmt(
            "eval.limits",
            ErrorKind::TooMuchMemory,
            span,
            format_args!("memory limit exceeded ({max} bytes)"),
        )
    }

    #[inline(always)]
    fn slot_ref(&self, id: ObjId) -> &Slot {
        let idx = id.0 as usize;
        let chunk_idx = idx >> 8;
        let in_chunk = idx & (CHUNK_SLOTS - 1);
        let chunks_len = self.chunks.len();
        assert!(
            chunk_idx < chunks_len
                && (chunk_idx + 1 < chunks_len || in_chunk < self.last_chunk_len),
            "invalid ObjId"
        );
        // SAFETY: 直上で `chunk_idx < chunks_len` かつ初期化済み範囲内を検査済み。
        unsafe { &*self.chunks.chunk_slot_ptr(chunk_idx).add(in_chunk) }
    }

    #[inline(always)]
    fn slot_mut(&mut self, id: ObjId) -> &mut Slot {
        let idx = id.0 as usize;
        let chunk_idx = idx >> 8;
        let in_chunk = idx & (CHUNK_SLOTS - 1);
        let chunks_len = self.chunks.len();
        assert!(
            chunk_idx < chunks_len
                && (chunk_idx + 1 < chunks_len || in_chunk < self.last_chunk_len),
            "invalid ObjId"
        );
        // SAFETY: 直上で `chunk_idx < chunks_len` かつ初期化済み範囲内を検査済み。
        unsafe { &mut *self.chunks.chunk_slot_ptr(chunk_idx).add(in_chunk) }
    }

    #[inline(always)]
    pub fn get(&self, id: ObjId) -> &Obj {
        match self.slot_ref(id) {
            Slot::Live { obj, .. } => obj,
            Slot::Free { .. } | Slot::Dying { .. } => panic!("dangling ObjId({})", id.0),
        }
    }

    #[inline(always)]
    pub fn retain_val(&mut self, val: RVal) {
        if let RVal::Obj(id) = val {
            self.retain(id);
        }
    }

    #[inline(always)]
    pub fn retain(&mut self, id: ObjId) {
        if let Slot::Live { rc, .. } = self.slot_mut(id) {
            *rc += 1;
        }
    }

    #[inline(always)]
    pub fn release_val(&mut self, val: RVal) {
        if let RVal::Obj(id) = val {
            if let Slot::Live { rc, .. } = self.slot_mut(id)
                && *rc > 1
            {
                *rc -= 1;
                return;
            }
            self.release(id);
        }
    }

    /// 参照カウントを減らし、0 になったオブジェクト群を侵入型 `Slot::Dying` リストで追加確保なしに反復解放する。
    ///
    /// 単一鎖（特に `Cons` リストや葉オブジェクト）は `Slot::Dying` を経由せず直接 `Slot::Free` へ戻すことで
    /// スロット書き換え回数を最小化する。
    pub fn release(&mut self, root: ObjId) {
        let mut pending: Option<u32> = None;
        let mut direct: Option<ObjId> = Some(root);
        loop {
            let obj = if let Some(id) = direct.take() {
                #[cfg(all(test, feature = "std-value"))]
                {
                    self.release_counts.references += 1;
                }
                let next_free = self.free_head;
                let slot = self.slot_mut(id);
                match slot {
                    Slot::Live { rc, .. } if *rc > 1 => {
                        *rc -= 1;
                        continue;
                    }
                    Slot::Live { .. } => {
                        let prev = std::mem::replace(slot, Slot::Free { next: next_free });
                        self.free_head = Some(id.0);
                        self.core.uncharge_obj(SLOT_BYTES);
                        let Slot::Live { obj, .. } = prev else {
                            unreachable!();
                        };
                        obj
                    }
                    Slot::Free { .. } | Slot::Dying { .. } => continue,
                }
            } else if let Some(id_u32) = pending {
                let id = ObjId(id_u32);
                let next_free = self.free_head;
                let prev = std::mem::replace(self.slot_mut(id), Slot::Free { next: next_free });
                self.free_head = Some(id_u32);
                let Slot::Dying { next, obj } = prev else {
                    unreachable!();
                };
                pending = next;
                self.core.uncharge_obj(SLOT_BYTES);
                obj
            } else {
                break;
            };

            #[cfg(all(test, feature = "std-value"))]
            self.release_counts.freed(&obj);

            if let Obj::Cons { head, tail, .. } = obj {
                if let RVal::Obj(h) = head {
                    if let RVal::Obj(t) = tail {
                        self.dec_and_enqueue(h, &mut pending);
                        direct = Some(t);
                    } else {
                        direct = Some(h);
                    }
                } else if let RVal::Obj(t) = tail {
                    direct = Some(t);
                }
            } else {
                self.free_obj_and_enqueue_children(obj, &mut pending);
            }
        }
    }

    #[inline]
    fn dec_and_enqueue(&mut self, id: ObjId, pending: &mut Option<u32>) {
        #[cfg(all(test, feature = "std-value"))]
        {
            self.release_counts.references += 1;
        }
        let slot = self.slot_mut(id);
        let became_zero = match slot {
            Slot::Live { rc, .. } => {
                *rc -= 1;
                *rc == 0
            }
            Slot::Free { .. } | Slot::Dying { .. } => false,
        };
        if became_zero {
            let prev = std::mem::replace(slot, Slot::Free { next: None });
            if let Slot::Live { obj, .. } = prev {
                *slot = Slot::Dying {
                    next: *pending,
                    obj,
                };
                *pending = Some(id.0);
            }
        }
    }

    fn free_obj_and_enqueue_children(&mut self, obj: Obj, pending: &mut Option<u32>) {
        match obj {
            Obj::Cons { head, tail, .. } => {
                if let RVal::Obj(id) = head {
                    self.dec_and_enqueue(id, pending);
                }
                if let RVal::Obj(id) = tail {
                    self.dec_and_enqueue(id, pending);
                }
            }
            Obj::Str(s) => {
                // SAFETY: `s` は `self.obj_heap` から確保されたブロック。
                unsafe { self.obj_heap.free(&mut self.core, s.block()) };
            }
            Obj::Array(xs) | Obj::Tuple(xs) => {
                for &x in xs.as_slice() {
                    if let RVal::Obj(id) = x {
                        self.dec_and_enqueue(id, pending);
                    }
                }
                // SAFETY: `xs` は `self.obj_heap` から確保されたブロック。
                unsafe { self.obj_heap.free(&mut self.core, xs.block()) };
            }
            Obj::Struct { desc, values } => {
                if let StructDescRef::Obj(desc_id) = desc {
                    self.dec_and_enqueue(desc_id, pending);
                }
                for &v in values.as_slice() {
                    if let RVal::Obj(id) = v {
                        self.dec_and_enqueue(id, pending);
                    }
                }
                // SAFETY: `values` は `self.obj_heap` から確保されたブロック。
                unsafe { self.obj_heap.free(&mut self.core, values.block()) };
            }
            Obj::Variant {
                type_name,
                variant,
                data,
            } => {
                if let Some(RVal::Obj(id)) = data {
                    self.dec_and_enqueue(id, pending);
                }
                // SAFETY: `type_name`・`variant` は `self.obj_heap` から確保されたブロック。
                unsafe {
                    self.obj_heap.free(&mut self.core, type_name.block());
                    self.obj_heap.free(&mut self.core, variant.block());
                }
            }
            Obj::Some(v) | Obj::Ok(v) | Obj::Err(v) => {
                if let RVal::Obj(id) = v {
                    self.dec_and_enqueue(id, pending);
                }
            }
            Obj::Closure { captures, .. } => {
                for &v in captures.as_slice() {
                    if let RVal::Obj(id) = v {
                        self.dec_and_enqueue(id, pending);
                    }
                }
                // SAFETY: `captures` は `self.obj_heap` から確保されたブロック。
                unsafe { self.obj_heap.free(&mut self.core, captures.block()) };
            }
            Obj::Module { name, exports } => {
                for exp in exports.as_slice() {
                    if let RVal::Obj(id) = exp.val {
                        self.dec_and_enqueue(id, pending);
                    }
                    // SAFETY: `exp.name` は `self.obj_heap` から確保されたブロック。
                    unsafe { self.obj_heap.free(&mut self.core, exp.name.block()) };
                }
                // SAFETY: `name`・`exports` は `self.obj_heap` から確保されたブロック。
                unsafe {
                    self.obj_heap.free(&mut self.core, name.block());
                    self.obj_heap.free(&mut self.core, exports.block());
                }
            }
            Obj::StructDesc { name, fields } => {
                for &f in fields.as_slice() {
                    // SAFETY: `f` は `self.obj_heap` から確保されたブロック。
                    unsafe { self.obj_heap.free(&mut self.core, f.block()) };
                }
                // SAFETY: `name`・`fields` は `self.obj_heap` から確保されたブロック。
                unsafe {
                    self.obj_heap.free(&mut self.core, name.block());
                    self.obj_heap.free(&mut self.core, fields.block());
                }
            }
        }
    }

    fn release_unallocated(&mut self, obj: Obj) {
        let mut pending: Option<u32> = None;
        self.free_obj_and_enqueue_children(obj, &mut pending);
        while let Some(id_u32) = pending {
            let id = ObjId(id_u32);
            let next_free = self.free_head;
            let prev = std::mem::replace(self.slot_mut(id), Slot::Free { next: next_free });
            self.free_head = Some(id_u32);
            if let Slot::Dying { next, obj } = prev {
                pending = next;
                self.core.uncharge_obj(SLOT_BYTES);
                #[cfg(all(test, feature = "std-value"))]
                self.release_counts.freed(&obj);
                self.free_obj_and_enqueue_children(obj, &mut pending);
            }
        }
    }

    #[inline(always)]
    pub fn alloc_obj(&mut self, obj: Obj, span: Span) -> Result<ObjId, ErrRecorded> {
        if !self.core.charge_obj(SLOT_BYTES) {
            return self.alloc_obj_oom(obj, span);
        }

        if let Some(free_idx) = self.free_head {
            let id = ObjId(free_idx);
            let slot = self.slot_mut(id);
            let next = match *slot {
                Slot::Free { next } => next,
                Slot::Live { .. } | Slot::Dying { .. } => unreachable!(),
            };
            *slot = Slot::Live { rc: 1, obj };
            self.free_head = next;
            return Ok(id);
        }

        if !self.chunks.is_empty() && self.last_chunk_len < CHUNK_SLOTS {
            let chunk_idx = self.chunks.len() - 1;
            let in_chunk = self.last_chunk_len;
            let global_idx = (chunk_idx * CHUNK_SLOTS + in_chunk) as u32;
            self.chunks
                .write_slot(chunk_idx, in_chunk, Slot::Live { rc: 1, obj });
            self.last_chunk_len += 1;
            return Ok(ObjId(global_idx));
        }

        self.alloc_obj_grow_chunk(obj, span)
    }

    #[cold]
    #[inline(never)]
    fn alloc_obj_oom(&mut self, obj: Obj, span: Span) -> Result<ObjId, ErrRecorded> {
        self.release_unallocated(obj);
        Err(self.oom_limit_error(span))
    }

    #[cold]
    #[inline(never)]
    fn alloc_obj_grow_chunk(&mut self, obj: Obj, span: Span) -> Result<ObjId, ErrRecorded> {
        let Some(layout) = Layout::array::<Slot>(CHUNK_SLOTS).ok() else {
            self.core.uncharge_obj(SLOT_BYTES);
            self.release_unallocated(obj);
            return Err(self.oom_error(span, "allocator exhausted while growing pool"));
        };
        let Some(raw) = self.core.alloc_raw(layout) else {
            self.core.uncharge_obj(SLOT_BYTES);
            self.release_unallocated(obj);
            return Err(self.oom_error(span, "allocator exhausted while growing pool"));
        };
        let buf = AllocBuf::<Slot>::from_raw(raw.cast::<Slot>(), CHUNK_SLOTS);
        if self.chunks.push(&mut self.core, buf).is_none() {
            // SAFETY: 直前に `self.core.alloc_raw(layout)` で確保した領域。
            unsafe { self.core.dealloc_raw(raw, layout) };
            self.core.uncharge_obj(SLOT_BYTES);
            self.release_unallocated(obj);
            return Err(self.oom_error(span, "allocator exhausted while growing pool"));
        }
        let chunk_idx = self.chunks.len() - 1;
        let global_idx = (chunk_idx * CHUNK_SLOTS) as u32;
        self.chunks
            .write_slot(chunk_idx, 0, Slot::Live { rc: 1, obj });
        self.last_chunk_len = 1;
        Ok(ObjId(global_idx))
    }
}

impl Drop for Pool<'_> {
    fn drop(&mut self) {
        self.chunks.destroy(&mut self.core, self.last_chunk_len);
        self.work.destroy(&mut self.core);
        self.obj_heap.destroy(&mut self.core);
        self.perm.destroy(&mut self.core);
    }
}
