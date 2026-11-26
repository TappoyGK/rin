//! 外部アロケータ上で動作する参照カウント付きオブジェクトプールと 3 領域メモリ管理を実装する。

use std::alloc::Layout;
use std::ptr::NonNull;

use crate::alloc::{AllocBuf, Allocator};
use crate::bytecode::{BuiltinId, FnProtoPtr};
use crate::error::{ErrRecorded, ErrorKind, Span};
use crate::mem::{
    AllocCore, MemoryConfig, ObjBlock, ObjHeap, PermArena, RawMap, RawStr, RawVec, WorkArena,
};
#[cfg(feature = "std-value")]
use crate::value::Value;

#[cfg(feature = "std-value")]
mod convert;
#[cfg(all(test, feature = "std-value"))]
mod release_tests;
#[cfg(feature = "std-value")]
mod shallow;

const CHUNK_SLOTS: usize = 256;

/// コンパイル出力（`Chunk`・`FnProto`・`NameTable` 等）の配置先領域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeRegion {
    /// 読み込み段階：永続領域 (`PermArena`) に配置する。
    Perm,
    /// 凍結後の評価：作業領域 (`WorkArena`) に配置し、評価終了時に一括巻き戻しする。
    Work,
}

impl<'a> Pool<'a> {
    pub(crate) fn alloc_code_str(&mut self, region: CodeRegion, s: &str) -> Option<RawStr> {
        match region {
            CodeRegion::Perm => RawStr::from_perm(&mut self.perm, &mut self.core, s),
            CodeRegion::Work => RawStr::from_work(&mut self.work, &mut self.core, s),
        }
    }

    pub(crate) fn alloc_code_val<T>(&mut self, region: CodeRegion, val: T) -> Option<NonNull<T>> {
        match region {
            CodeRegion::Perm => self.perm.alloc_val(&mut self.core, val),
            CodeRegion::Work => self.work.alloc_val(&mut self.core, val),
        }
    }

    pub(crate) fn push_code<T>(
        &mut self,
        region: CodeRegion,
        vec: &mut RawVec<T>,
        val: T,
    ) -> Option<()> {
        match region {
            CodeRegion::Perm => vec.push_perm(&mut self.perm, &mut self.core, val),
            CodeRegion::Work => vec.push_work(&mut self.work, &mut self.core, val),
        }
    }

    pub(crate) fn extend_code_slice<T: Copy>(
        &mut self,
        region: CodeRegion,
        vec: &mut RawVec<T>,
        src: &[T],
    ) -> Option<()> {
        match region {
            CodeRegion::Perm => vec.extend_from_slice_perm(&mut self.perm, &mut self.core, src),
            CodeRegion::Work => vec.extend_from_slice_work(&mut self.work, &mut self.core, src),
        }
    }
}

/// 内部プール内の参照カウント付きオブジェクトへのハンドル。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObjId(pub u32);

/// 変数・スタック・プールオブジェクトに格納する軽量な実行時値。
#[derive(Debug, Clone, Copy)]
pub(crate) enum RVal {
    Void,
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    None,
    EmptyList,
    Builtin(BuiltinId),
    Obj(ObjId),
}

impl RVal {
    pub fn type_name(self, pool: &Pool<'_>) -> &'static str {
        match self {
            RVal::Void => "void",
            RVal::Int(_) => "i64",
            RVal::Float(_) => "f64",
            RVal::Bool(_) => "bool",
            RVal::Char(_) => "char",
            RVal::None => "option",
            RVal::EmptyList => "list",
            RVal::Builtin(_) => "fn",
            RVal::Obj(id) => match pool.get(id) {
                Obj::Cons { .. } => "list",
                Obj::Array(_) => "array",
                Obj::Str(_) => "string",
                Obj::Tuple(_) => "tuple",
                Obj::Struct { .. } => "struct",
                Obj::Variant { .. } => "variant",
                Obj::Some(_) => "option",
                Obj::Ok(_) | Obj::Err(_) => "result",
                Obj::Closure { .. } => "fn",
                Obj::Module { .. } => "module",
                Obj::StructDesc { .. } => "struct_desc",
            },
        }
    }
}

/// オブジェクト領域 (`ObjHeap`) 上に確保された固定長配列。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ObjSlice<T: Copy> {
    ptr: NonNull<T>,
    len: usize,
    block_size: usize,
}

impl<T: Copy> ObjSlice<T> {
    pub const fn empty() -> Self {
        Self {
            ptr: NonNull::dangling(),
            len: 0,
            block_size: 0,
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
    pub fn as_slice(&self) -> &[T] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: `0..self.len` は初期化済みで有効。
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.len == 0 {
            return &mut [];
        }
        // SAFETY: `0..self.len` は初期化済みで排他参照。
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    #[inline]
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.as_slice().iter()
    }

    #[inline]
    fn block(self) -> ObjBlock {
        ObjBlock {
            ptr: self.ptr.cast::<u8>(),
            actual_size: self.block_size,
        }
    }
}

impl<T: Copy> std::ops::Index<usize> for ObjSlice<T> {
    type Output = T;
    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        &self.as_slice()[index]
    }
}

/// オブジェクト領域 (`ObjHeap`) 上に確保された UTF-8 文字列。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ObjStr {
    ptr: NonNull<u8>,
    len: usize,
    block_size: usize,
}

impl ObjStr {
    pub const fn empty() -> Self {
        Self {
            ptr: NonNull::dangling(),
            len: 0,
            block_size: 0,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        if self.len == 0 {
            return "";
        }
        // SAFETY: 構築時に有効な UTF-8 文字列からコピーしている。
        unsafe {
            let bytes = std::slice::from_raw_parts(self.ptr.as_ptr(), self.len);
            std::str::from_utf8_unchecked(bytes)
        }
    }

    #[inline]
    fn block(self) -> ObjBlock {
        ObjBlock {
            ptr: self.ptr,
            actual_size: self.block_size,
        }
    }
}

/// モジュールの公開要素（名前辞書順ソート済み配列で保持する）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ModuleExport {
    pub name: ObjStr,
    pub val: RVal,
}

/// 構造体記述子への参照。読み込み段階では永続領域にインターンし、凍結後の新規構造体はオブジェクト領域に置く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructDescRef {
    Perm(u32),
    Obj(ObjId),
}

struct PermStructDesc {
    name: RawStr,
    fields: RawVec<RawStr>,
}

struct StructDescTable {
    descs: RawVec<PermStructDesc>,
    by_hash: RawMap<u32, u32>,
}

impl StructDescTable {
    const fn new() -> Self {
        Self {
            descs: RawVec::new(),
            by_hash: RawMap::new(),
        }
    }

    fn hash_desc(name: &str, sorted_fields: &[&str]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mix = |h: &mut u64, bytes: &[u8]| {
            for &b in bytes {
                *h ^= b as u64;
                *h = h.wrapping_mul(0x100000001b3);
            }
            *h ^= 0xff;
            *h = h.wrapping_mul(0x100000001b3);
        };
        mix(&mut h, name.as_bytes());
        for &f in sorted_fields {
            mix(&mut h, f.as_bytes());
        }
        h
    }

    fn matches(&self, id: u32, name: &str, sorted_fields: &[&str]) -> bool {
        let desc = &self.descs.as_slice()[id as usize];
        if desc.name.as_str() != name || desc.fields.len() != sorted_fields.len() {
            return false;
        }
        desc.fields
            .as_slice()
            .iter()
            .zip(sorted_fields.iter())
            .all(|(a, b)| a.as_str() == *b)
    }

    fn find(&self, name: &str, sorted_fields: &[&str]) -> Option<u32> {
        let h = Self::hash_desc(name, sorted_fields);
        self.by_hash
            .get_by(h, |&id| self.matches(id, name, sorted_fields))
    }

    fn intern(
        &mut self,
        perm: &mut PermArena,
        core: &mut AllocCore<'_>,
        name: &str,
        sorted_fields: &[&str],
    ) -> Option<u32> {
        if let Some(id) = self.find(name, sorted_fields) {
            return Some(id);
        }
        if perm.is_frozen() {
            return None;
        }
        let h = Self::hash_desc(name, sorted_fields);
        let perm_name = RawStr::from_perm(perm, core, name)?;
        let mut perm_fields = RawVec::new();
        perm_fields.reserve_perm(perm, core, sorted_fields.len())?;
        for &f in sorted_fields {
            let fs = RawStr::from_perm(perm, core, f)?;
            perm_fields.push_perm(perm, core, fs)?;
        }
        let id = self.descs.len() as u32;
        self.descs.push_perm(
            perm,
            core,
            PermStructDesc {
                name: perm_name,
                fields: perm_fields,
            },
        )?;
        let descs_ptr = self.descs.as_slice().as_ptr();
        let descs_len = self.descs.len();
        self.by_hash.insert_perm(perm, core, h, id, id, |&a, &b| {
            // SAFETY: `descs` は直前に確定済み。
            let slice = unsafe { std::slice::from_raw_parts(descs_ptr, descs_len) };
            let da = &slice[a as usize];
            let db = &slice[b as usize];
            da.name.as_str() == db.name.as_str()
                && da.fields.len() == db.fields.len()
                && da
                    .fields
                    .as_slice()
                    .iter()
                    .zip(db.fields.as_slice().iter())
                    .all(|(x, y)| x.as_str() == y.as_str())
        })?;
        Some(id)
    }
}

pub(crate) enum Obj {
    Cons {
        head: RVal,
        tail: RVal,
        len: usize,
    },
    Array(ObjSlice<RVal>),
    Str(ObjStr),
    Tuple(ObjSlice<RVal>),
    Struct {
        desc: StructDescRef,
        values: ObjSlice<RVal>,
    },
    Variant {
        type_name: ObjStr,
        variant: ObjStr,
        data: Option<RVal>,
    },
    Some(RVal),
    Ok(RVal),
    Err(RVal),
    Closure {
        proto: FnProtoPtr,
        captures: ObjSlice<RVal>,
    },
    Module {
        name: ObjStr,
        exports: ObjSlice<ModuleExport>,
    },
    StructDesc {
        name: ObjStr,
        fields: ObjSlice<ObjStr>,
    },
}

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

    /// オブジェクト領域上に文字列ペイロードを確保する。
    pub(crate) fn alloc_obj_str(&mut self, s: &str, span: Span) -> Result<ObjStr, ErrRecorded> {
        if s.is_empty() {
            return Ok(ObjStr::empty());
        }
        let Some(block) = self.obj_heap.alloc(&mut self.core, s.len(), 1) else {
            return Err(self.oom_limit_error(span));
        };
        // SAFETY: `block.actual_size >= s.len()`。
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), block.ptr.as_ptr(), s.len()) };
        Ok(ObjStr {
            ptr: block.ptr,
            len: s.len(),
            block_size: block.actual_size,
        })
    }

    /// オブジェクト領域上に `Copy` 要素スライスを確保してコピーする。
    pub(crate) fn alloc_obj_slice<T: Copy>(
        &mut self,
        src: &[T],
        span: Span,
    ) -> Result<ObjSlice<T>, ErrRecorded> {
        if src.is_empty() || std::mem::size_of::<T>() == 0 {
            return Ok(ObjSlice::empty());
        }
        let Some(layout) = Layout::array::<T>(src.len()).ok() else {
            return Err(self.oom_error(span, "memory limit exceeded"));
        };
        let Some(block) = self
            .obj_heap
            .alloc(&mut self.core, layout.size(), layout.align())
        else {
            return Err(self.oom_limit_error(span));
        };
        let ptr = block.ptr.cast::<T>();
        // SAFETY: `block.actual_size >= layout.size()` かつ非重複。
        unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), ptr.as_ptr(), src.len()) };
        Ok(ObjSlice {
            ptr,
            len: src.len(),
            block_size: block.actual_size,
        })
    }

    /// `Obj::Str` を確保して `ObjId` を返す。
    pub fn alloc_str(&mut self, s: &str, span: Span) -> Result<ObjId, ErrRecorded> {
        let os = self.alloc_obj_str(s, span)?;
        self.alloc_obj(Obj::Str(os), span)
    }

    /// 所有権を引き受けた `RVal` スライスから `Obj::Array` を確保する。失敗時は全要素を解放する。
    pub fn alloc_array_owned(&mut self, items: &[RVal], span: Span) -> Result<ObjId, ErrRecorded> {
        match self.alloc_obj_slice(items, span) {
            Ok(slice) => self.alloc_obj(Obj::Array(slice), span),
            Err(e) => {
                for &v in items {
                    self.release_val(v);
                }
                Err(e)
            }
        }
    }

    /// 所有権を引き受けた `RVal` スライスから `Obj::Tuple` を確保する。失敗時は全要素を解放する。
    pub fn alloc_tuple_owned(&mut self, items: &[RVal], span: Span) -> Result<ObjId, ErrRecorded> {
        match self.alloc_obj_slice(items, span) {
            Ok(slice) => self.alloc_obj(Obj::Tuple(slice), span),
            Err(e) => {
                for &v in items {
                    self.release_val(v);
                }
                Err(e)
            }
        }
    }

    /// `Obj::Variant` を確保する。失敗時は `data` を解放する。
    pub fn alloc_variant_owned(
        &mut self,
        type_name: &str,
        variant: &str,
        data: Option<RVal>,
        span: Span,
    ) -> Result<ObjId, ErrRecorded> {
        let tn = match self.alloc_obj_str(type_name, span) {
            Ok(s) => s,
            Err(e) => {
                if let Some(d) = data {
                    self.release_val(d);
                }
                return Err(e);
            }
        };
        let vn = match self.alloc_obj_str(variant, span) {
            Ok(s) => s,
            Err(e) => {
                // SAFETY: 直前に確保した `tn`。
                unsafe { self.obj_heap.free(&mut self.core, tn.block()) };
                if let Some(d) = data {
                    self.release_val(d);
                }
                return Err(e);
            }
        };
        self.alloc_obj(
            Obj::Variant {
                type_name: tn,
                variant: vn,
                data,
            },
            span,
        )
    }

    /// 所有権を引き受けた `captures` から `Obj::Closure` を確保する。失敗時は全捕捉値を解放する。
    pub fn alloc_closure_owned(
        &mut self,
        proto: FnProtoPtr,
        captures: &[RVal],
        span: Span,
    ) -> Result<ObjId, ErrRecorded> {
        match self.alloc_obj_slice(captures, span) {
            Ok(slice) => self.alloc_obj(
                Obj::Closure {
                    proto,
                    captures: slice,
                },
                span,
            ),
            Err(e) => {
                for &v in captures {
                    self.release_val(v);
                }
                Err(e)
            }
        }
    }

    /// 構造体記述子（フィールド名は辞書順ソート済み）をインターンまたは確保する。
    pub fn intern_struct_desc(
        &mut self,
        name: &str,
        sorted_fields: &[&str],
        span: Span,
    ) -> Result<StructDescRef, ErrRecorded> {
        if let Some(id) = self.struct_descs.find(name, sorted_fields) {
            return Ok(StructDescRef::Perm(id));
        }
        if !self.perm.is_frozen() {
            if let Some(id) =
                self.struct_descs
                    .intern(&mut self.perm, &mut self.core, name, sorted_fields)
            {
                return Ok(StructDescRef::Perm(id));
            }
            return Err(self.oom_limit_error(span));
        }
        // 凍結後の新規構造体はオブジェクト領域に参照カウント付き記述子として置く。
        let os_name = self.alloc_obj_str(name, span)?;
        let mut fields_slice =
            match self.alloc_obj_slice_uninit::<ObjStr>(sorted_fields.len(), span) {
                Ok(s) => s,
                Err(e) => {
                    // SAFETY: 直前に確保した `os_name`。
                    unsafe { self.obj_heap.free(&mut self.core, os_name.block()) };
                    return Err(e);
                }
            };
        let mut init_len = 0usize;
        for &f in sorted_fields {
            match self.alloc_obj_str(f, span) {
                Ok(fs) => {
                    fields_slice.as_mut_slice()[init_len] = fs;
                    init_len += 1;
                }
                Err(e) => {
                    for &prev in &fields_slice.as_slice()[..init_len] {
                        // SAFETY: 初期化済みの `ObjStr`。
                        unsafe { self.obj_heap.free(&mut self.core, prev.block()) };
                    }
                    // SAFETY: 直前に確保した領域。
                    unsafe {
                        self.obj_heap.free(&mut self.core, fields_slice.block());
                        self.obj_heap.free(&mut self.core, os_name.block());
                    }
                    return Err(e);
                }
            }
        }
        let desc_id = self.alloc_obj(
            Obj::StructDesc {
                name: os_name,
                fields: fields_slice,
            },
            span,
        )?;
        Ok(StructDescRef::Obj(desc_id))
    }

    fn alloc_obj_slice_uninit<T: Copy>(
        &mut self,
        len: usize,
        span: Span,
    ) -> Result<ObjSlice<T>, ErrRecorded> {
        if len == 0 || std::mem::size_of::<T>() == 0 {
            return Ok(ObjSlice::empty());
        }
        let Some(layout) = Layout::array::<T>(len).ok() else {
            return Err(self.oom_error(span, "memory limit exceeded"));
        };
        let Some(block) = self
            .obj_heap
            .alloc(&mut self.core, layout.size(), layout.align())
        else {
            return Err(self.oom_limit_error(span));
        };
        // SAFETY: `block.actual_size >= layout.size()`。ゼロ初期化しておく。
        unsafe { std::ptr::write_bytes(block.ptr.as_ptr(), 0, layout.size()) };
        Ok(ObjSlice {
            ptr: block.ptr.cast::<T>(),
            len,
            block_size: block.actual_size,
        })
    }

    /// 構造体記述子と所有権を引き受けた値配列（辞書順フィールド対応）から `Obj::Struct` を確保する。
    pub fn alloc_struct_owned(
        &mut self,
        desc: StructDescRef,
        values: &[RVal],
        span: Span,
    ) -> Result<ObjId, ErrRecorded> {
        match self.alloc_obj_slice(values, span) {
            Ok(slice) => self.alloc_obj(
                Obj::Struct {
                    desc,
                    values: slice,
                },
                span,
            ),
            Err(e) => {
                if let StructDescRef::Obj(id) = desc {
                    self.release(id);
                }
                for &v in values {
                    self.release_val(v);
                }
                Err(e)
            }
        }
    }

    /// 辞書順ソート済みの `(名前, 所有 RVal)` 一覧から `Obj::Module` を確保する。失敗時は全要素を解放する。
    pub fn alloc_module_owned(
        &mut self,
        name: &str,
        sorted_exports: &[(&str, RVal)],
        span: Span,
    ) -> Result<ObjId, ErrRecorded> {
        let mod_name = match self.alloc_obj_str(name, span) {
            Ok(s) => s,
            Err(e) => {
                for &(_, v) in sorted_exports {
                    self.release_val(v);
                }
                return Err(e);
            }
        };
        let mut slice =
            match self.alloc_obj_slice_uninit::<ModuleExport>(sorted_exports.len(), span) {
                Ok(s) => s,
                Err(e) => {
                    // SAFETY: 直前に確保した `mod_name`。
                    unsafe { self.obj_heap.free(&mut self.core, mod_name.block()) };
                    for &(_, v) in sorted_exports {
                        self.release_val(v);
                    }
                    return Err(e);
                }
            };
        let mut init_len = 0usize;
        for &(k, v) in sorted_exports {
            match self.alloc_obj_str(k, span) {
                Ok(ks) => {
                    slice.as_mut_slice()[init_len] = ModuleExport { name: ks, val: v };
                    init_len += 1;
                }
                Err(e) => {
                    for exp in &slice.as_slice()[..init_len] {
                        // SAFETY: 初期化済みの `ObjStr`。
                        unsafe { self.obj_heap.free(&mut self.core, exp.name.block()) };
                    }
                    // SAFETY: 直前に確保した領域。
                    unsafe {
                        self.obj_heap.free(&mut self.core, slice.block());
                        self.obj_heap.free(&mut self.core, mod_name.block());
                    }
                    for &(_, rv) in sorted_exports {
                        self.release_val(rv);
                    }
                    return Err(e);
                }
            }
        }
        self.alloc_obj(
            Obj::Module {
                name: mod_name,
                exports: slice,
            },
            span,
        )
    }

    pub fn struct_desc_name(&self, desc: StructDescRef) -> &str {
        match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize]
                .name
                .as_str(),
            StructDescRef::Obj(id) => match self.get(id) {
                Obj::StructDesc { name, .. } => name.as_str(),
                _ => unreachable!(),
            },
        }
    }

    #[cfg(feature = "std-value")]
    pub fn struct_desc_field_count(&self, desc: StructDescRef) -> usize {
        match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize].fields.len(),
            StructDescRef::Obj(id) => match self.get(id) {
                Obj::StructDesc { fields, .. } => fields.len(),
                _ => unreachable!(),
            },
        }
    }

    pub fn struct_desc_field_name(&self, desc: StructDescRef, idx: usize) -> &str {
        match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize]
                .fields
                .as_slice()[idx]
                .as_str(),
            StructDescRef::Obj(id) => match self.get(id) {
                Obj::StructDesc { fields, .. } => fields[idx].as_str(),
                _ => unreachable!(),
            },
        }
    }

    pub fn struct_desc_find_field(&self, desc: StructDescRef, field: &str) -> Option<usize> {
        match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize]
                .fields
                .as_slice()
                .binary_search_by(|f| f.as_str().cmp(field))
                .ok(),
            StructDescRef::Obj(id) => match self.get(id) {
                Obj::StructDesc { fields, .. } => fields
                    .as_slice()
                    .binary_search_by(|f| f.as_str().cmp(field))
                    .ok(),
                _ => unreachable!(),
            },
        }
    }

    pub fn struct_desc_retain(&mut self, desc: StructDescRef) {
        if let StructDescRef::Obj(id) = desc {
            self.retain(id);
        }
    }

    pub(crate) fn copy_struct_desc_name_to_work(&mut self, desc: StructDescRef) -> Option<RawStr> {
        let name = match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize]
                .name
                .as_str(),
            StructDescRef::Obj(id) => {
                let idx = id.0 as usize;
                let chunk_idx = idx / CHUNK_SLOTS;
                let in_chunk = idx % CHUNK_SLOTS;
                let len = if chunk_idx + 1 == self.chunks.len() {
                    self.last_chunk_len
                } else {
                    CHUNK_SLOTS
                };
                match &self.chunks.as_slice()[chunk_idx].as_slice(len)[in_chunk] {
                    Slot::Live {
                        obj: Obj::StructDesc { name, .. },
                        ..
                    } => name.as_str(),
                    _ => unreachable!(),
                }
            }
        };
        RawStr::from_work(&mut self.work, &mut self.core, name)
    }

    pub(crate) fn copy_struct_desc_field_to_work(
        &mut self,
        desc: StructDescRef,
        field_idx: usize,
    ) -> Option<RawStr> {
        let fname = match desc {
            StructDescRef::Perm(id) => self.struct_descs.descs.as_slice()[id as usize]
                .fields
                .as_slice()[field_idx]
                .as_str(),
            StructDescRef::Obj(id) => {
                let idx = id.0 as usize;
                let chunk_idx = idx / CHUNK_SLOTS;
                let in_chunk = idx % CHUNK_SLOTS;
                let len = if chunk_idx + 1 == self.chunks.len() {
                    self.last_chunk_len
                } else {
                    CHUNK_SLOTS
                };
                match &self.chunks.as_slice()[chunk_idx].as_slice(len)[in_chunk] {
                    Slot::Live {
                        obj: Obj::StructDesc { fields, .. },
                        ..
                    } => fields[field_idx].as_str(),
                    _ => unreachable!(),
                }
            }
        };
        RawStr::from_work(&mut self.work, &mut self.core, fname)
    }

    #[cold]
    #[inline(never)]
    pub fn err_struct_missing_field(
        &mut self,
        rule: &'static str,
        span: Span,
        desc: StructDescRef,
        field: &str,
    ) -> ErrRecorded {
        use std::fmt::Write;
        let out = self.core.error.begin_msg(rule, ErrorKind::Runtime, span);
        match desc {
            StructDescRef::Perm(id) => {
                let d = &self.struct_descs.descs.as_slice()[id as usize];
                let _ = write!(out, "{} has no field '{field}'", d.name.as_str());
                crate::diagnostic::write_available(
                    out,
                    "fields",
                    d.fields.as_slice().iter().map(RawStr::as_str),
                );
            }
            StructDescRef::Obj(id) => {
                let idx = id.0 as usize;
                let chunk_idx = idx / CHUNK_SLOTS;
                let in_chunk = idx % CHUNK_SLOTS;
                let len = if chunk_idx + 1 == self.chunks.len() {
                    self.last_chunk_len
                } else {
                    CHUNK_SLOTS
                };
                let slot = &self.chunks.as_slice()[chunk_idx].as_slice(len)[in_chunk];
                let Slot::Live {
                    obj: Obj::StructDesc { name, fields },
                    ..
                } = slot
                else {
                    unreachable!();
                };
                let _ = write!(out, "{} has no field '{field}'", name.as_str());
                crate::diagnostic::write_available(
                    out,
                    "fields",
                    fields.as_slice().iter().map(ObjStr::as_str),
                );
            }
        }
        ErrRecorded
    }

    pub fn find_module_export(exports: ObjSlice<ModuleExport>, name: &str) -> Option<RVal> {
        exports
            .as_slice()
            .binary_search_by(|e| e.name.as_str().cmp(name))
            .ok()
            .map(|idx| exports[idx].val)
    }

    pub fn alloc_cons(&mut self, head: RVal, tail: RVal, span: Span) -> Result<RVal, ErrRecorded> {
        let tail_len = self.list_len(tail);
        self.retain_val(head);
        self.retain_val(tail);
        let id = self.alloc_obj(
            Obj::Cons {
                head,
                tail,
                len: tail_len + 1,
            },
            span,
        )?;
        Ok(RVal::Obj(id))
    }

    pub fn list_len(&self, val: RVal) -> usize {
        match val {
            RVal::EmptyList => 0,
            RVal::Obj(id) => match self.get(id) {
                Obj::Cons { len, .. } => *len,
                _ => 0,
            },
            _ => 0,
        }
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
