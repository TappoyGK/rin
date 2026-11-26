//! 外部 `Allocator` の上だけで動作するための 3 領域メモリ管理と自前コンテナを提供する。
//!
//! すべての領域は [`AllocCore`] を通じて単一の `Allocator` と `memory_bytes` 上限を共有する。
//!
//! - [`PermArena`]（永続領域）: `Chunk`・`FnProto`・シンボル表・`StructDesc`・モジュール等。追記のみで凍結後は読み取り専用。
//! - [`ObjHeap`]（オブジェクト領域）: 評価中に作られる `Obj` 本体と可変長ペイロード。サイズ別フリーリストで再利用する。
//! - [`WorkArena`]（作業領域）: AST・コンパイラ一時データ・値スタック・呼び出しフレーム。評価終了時に先頭へ巻き戻す。

mod arena;
mod obj_heap;
mod raw_map;
mod raw_str;
mod raw_vec;
#[cfg(test)]
mod tests;

use std::alloc::Layout;
use std::fmt;
use std::ptr::NonNull;

use crate::alloc::Allocator;
use crate::error::{ErrRecorded, ErrorKind, ErrorStore, Span};

#[allow(unused_imports)]
pub(crate) use arena::{PermArena, WorkArena, WorkMark};
#[allow(unused_imports)]
pub(crate) use obj_heap::{ObjBlock, ObjHeap};
#[allow(unused_imports)]
pub(crate) use raw_map::{RawMap, fnv1a_bytes, fnv1a_u32};
#[allow(unused_imports)]
pub(crate) use raw_str::RawStr;
#[allow(unused_imports)]
pub(crate) use raw_vec::{RawVec, WorkBox};

/// 領域が `Allocator` から取得するチャンクの大きさ設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionConfig {
    /// 最初に確保するチャンクの基準サイズ（バイト）。
    pub initial_block: usize,
    /// チャンク拡張時の上限サイズ（バイト）。
    pub max_block: usize,
}

impl RegionConfig {
    pub const fn new(initial_block: usize, max_block: usize) -> Self {
        Self {
            initial_block,
            max_block,
        }
    }

    pub(crate) fn normalized(self) -> Self {
        let initial = self.initial_block.max(256);
        let max = self.max_block.max(initial);
        Self {
            initial_block: initial,
            max_block: max,
        }
    }
}

/// 永続・オブジェクト・作業の 3 領域のブロック設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryConfig {
    pub perm: RegionConfig,
    pub obj: RegionConfig,
    pub work: RegionConfig,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            perm: RegionConfig::new(4 * 1024, 64 * 1024),
            obj: RegionConfig::new(4 * 1024, 64 * 1024),
            work: RegionConfig::new(4 * 1024, 64 * 1024),
        }
    }
}

/// 3 領域が共有する外部 `Allocator` とバイト数会計、およびゼロヒープエラーストア。
pub(crate) struct AllocCore<'a> {
    alloc: &'a mut dyn Allocator,
    max_bytes: usize,
    perm_bytes: usize,
    obj_bytes: usize,
    work_bytes: usize,
    raw_allocated_bytes: usize,
    pub(crate) error: ErrorStore,
}

impl<'a> AllocCore<'a> {
    pub fn new(alloc: &'a mut dyn Allocator, max_bytes: usize) -> Self {
        Self {
            alloc,
            max_bytes,
            perm_bytes: 0,
            obj_bytes: 0,
            work_bytes: 0,
            raw_allocated_bytes: 0,
            error: ErrorStore::new(),
        }
    }

    #[cold]
    pub(crate) fn err_str(
        &mut self,
        rule: &'static str,
        kind: ErrorKind,
        at: Span,
        msg: &str,
    ) -> ErrRecorded {
        self.error.set_str(rule, kind, at, msg)
    }

    #[cold]
    pub(crate) fn err_fmt(
        &mut self,
        rule: &'static str,
        kind: ErrorKind,
        at: Span,
        args: fmt::Arguments<'_>,
    ) -> ErrRecorded {
        self.error.set_fmt(rule, kind, at, args)
    }

    #[cold]
    pub(crate) fn err_oom(&mut self, at: Span) -> ErrRecorded {
        let max = self.max_bytes;
        self.error.set_fmt(
            "eval.limits",
            ErrorKind::TooMuchMemory,
            at,
            format_args!("memory limit exceeded ({max} bytes)"),
        )
    }

    #[inline]
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    #[inline]
    pub fn perm_bytes(&self) -> usize {
        self.perm_bytes
    }

    #[inline]
    pub fn obj_bytes(&self) -> usize {
        self.obj_bytes
    }

    #[inline]
    pub fn work_bytes(&self) -> usize {
        self.work_bytes
    }

    /// 現在生存している論理確保バイト数（永続 + オブジェクト + 作業）。
    #[inline(always)]
    pub fn total_live_bytes(&self) -> usize {
        self.perm_bytes
            .saturating_add(self.obj_bytes)
            .saturating_add(self.work_bytes)
    }

    /// `Allocator` から現在確保中の生チャンク総バイト数。
    #[inline]
    pub fn raw_allocated_bytes(&self) -> usize {
        self.raw_allocated_bytes
    }

    /// 永続領域の論理使用量を `bytes` だけ増やせるか検査して予約する。
    #[inline(always)]
    pub(crate) fn charge_perm(&mut self, bytes: usize) -> bool {
        let next = self.total_live_bytes().saturating_add(bytes);
        if next > self.max_bytes {
            return false;
        }
        self.perm_bytes += bytes;
        true
    }

    #[inline(always)]
    pub(crate) fn uncharge_perm(&mut self, bytes: usize) {
        self.perm_bytes = self.perm_bytes.saturating_sub(bytes);
    }

    /// オブジェクト領域の論理使用量を `bytes` だけ増やせるか検査して予約する。
    #[inline(always)]
    pub(crate) fn charge_obj(&mut self, bytes: usize) -> bool {
        let next = self.total_live_bytes().saturating_add(bytes);
        if next > self.max_bytes {
            return false;
        }
        self.obj_bytes += bytes;
        true
    }

    #[inline(always)]
    pub(crate) fn uncharge_obj(&mut self, bytes: usize) {
        self.obj_bytes = self.obj_bytes.saturating_sub(bytes);
    }

    /// 作業領域の論理使用量を `bytes` だけ増やせるか検査して予約する。
    #[inline(always)]
    pub(crate) fn charge_work(&mut self, bytes: usize) -> bool {
        let next = self.total_live_bytes().saturating_add(bytes);
        if next > self.max_bytes {
            return false;
        }
        self.work_bytes += bytes;
        true
    }

    #[inline(always)]
    pub(crate) fn set_work_bytes(&mut self, bytes: usize) {
        self.work_bytes = bytes;
    }

    /// 外部 `Allocator` から生ブロックを確保する。
    pub(crate) fn alloc_raw(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        if layout.size() == 0 {
            return Some(NonNull::dangling());
        }
        let ptr = self.alloc.alloc(layout)?;
        self.raw_allocated_bytes = self.raw_allocated_bytes.saturating_add(layout.size());
        Some(ptr)
    }

    /// 希望サイズ `desired` でチャンク確保を試み、失敗したら `min_size` まで縮小して再試行する。
    pub(crate) fn alloc_chunk_flexible(
        &mut self,
        min_size: usize,
        desired: usize,
        align: usize,
    ) -> Option<(NonNull<u8>, Layout)> {
        let mut size = desired.max(min_size);
        loop {
            if let Ok(layout) = Layout::from_size_align(size, align)
                && let Some(ptr) = self.alloc_raw(layout)
            {
                return Some((ptr, layout));
            }
            if size <= min_size {
                return None;
            }
            size = (size / 2).max(min_size);
        }
    }

    /// 外部 `Allocator` へ生ブロックを返却する。
    ///
    /// # Safety
    /// `ptr` はこの `AllocCore` の `alloc_raw` で `layout` により確保された未解放ブロックでなければならない。
    pub(crate) unsafe fn dealloc_raw(&mut self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() == 0 {
            return;
        }
        self.raw_allocated_bytes = self.raw_allocated_bytes.saturating_sub(layout.size());
        // SAFETY: 呼び出し側が `alloc_raw` 由来のポインタ・レイアウトであることを保証する。
        unsafe { self.alloc.dealloc(ptr, layout) };
    }
}

#[inline]
pub(crate) fn align_up(offset: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    let mask = align - 1;
    offset.checked_add(mask).map(|v| v & !mask)
}
