//! 永続領域 (`PermArena`) と作業領域 (`WorkArena`) のバンプ確保チャンク実装。

use std::alloc::Layout;
use std::ptr::NonNull;

use super::{AllocCore, RegionConfig, align_up};

/// チャンク先頭に埋め込むメタデータ。チャンク自体をリンクリストで管理し、外部配列を不要にする。
#[repr(C)]
struct ChunkHeader {
    next: Option<NonNull<ChunkHeader>>,
    layout: Layout,
    /// ヘッダ末尾からチャンク末尾までの利用可能バイト数。
    data_cap: usize,
    /// このチャンク内で現在使用済みのバイト数（データ先頭からのオフセット）。
    used: usize,
}

const CHUNK_ALIGN: usize = 16;

#[inline]
fn header_padded_size() -> usize {
    align_up(std::mem::size_of::<ChunkHeader>(), CHUNK_ALIGN).expect("header size overflow")
}

#[inline]
unsafe fn chunk_data_ptr(header: NonNull<ChunkHeader>) -> *mut u8 {
    // SAFETY: チャンクは `header_padded_size() + data_cap` バイトで確保されている。
    unsafe { header.as_ptr().cast::<u8>().add(header_padded_size()) }
}

fn alloc_chunk(
    core: &mut AllocCore<'_>,
    min_data: usize,
    desired_data: usize,
    align: usize,
) -> Option<NonNull<ChunkHeader>> {
    let hdr_size = header_padded_size();
    let chunk_align = CHUNK_ALIGN.max(align);
    let min_total = hdr_size.checked_add(min_data)?;
    let desired_total = hdr_size.checked_add(desired_data.max(min_data))?;
    let (raw, layout) = core.alloc_chunk_flexible(min_total, desired_total, chunk_align)?;
    let data_cap = layout.size() - hdr_size;
    let header_ptr = raw.cast::<ChunkHeader>();
    // SAFETY: `raw` は `chunk_align >= align_of::<ChunkHeader>()` かつ `layout.size() >= hdr_size`。
    unsafe {
        header_ptr.as_ptr().write(ChunkHeader {
            next: None,
            layout,
            data_cap,
            used: 0,
        });
    }
    Some(header_ptr)
}

unsafe fn destroy_chunk_chain(core: &mut AllocCore<'_>, mut head: Option<NonNull<ChunkHeader>>) {
    while let Some(hdr) = head {
        // SAFETY: `hdr` は `alloc_chunk` で確保された有効なヘッダ。
        let (next, layout) = unsafe {
            let r = hdr.as_ref();
            (r.next, r.layout)
        };
        // SAFETY: チャンク全体は `core.alloc_raw(layout)` 由来。
        unsafe { core.dealloc_raw(hdr.cast::<u8>(), layout) };
        head = next;
    }
}

/// 追記専用の永続領域。凍結後は書き込みを禁止する。
pub(crate) struct PermArena {
    head: Option<NonNull<ChunkHeader>>,
    tail: Option<NonNull<ChunkHeader>>,
    config: RegionConfig,
    next_block_size: usize,
    frozen: bool,
}

impl PermArena {
    pub fn new(config: RegionConfig) -> Self {
        let config = config.normalized();
        Self {
            head: None,
            tail: None,
            config,
            next_block_size: config.initial_block,
            frozen: false,
        }
    }

    #[inline]
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    pub fn freeze(&mut self) {
        self.frozen = true;
    }

    /// 永続領域から `layout` の生領域を確保する。凍結後または上限超過時は `None`。
    pub fn alloc_raw(&mut self, core: &mut AllocCore<'_>, layout: Layout) -> Option<NonNull<u8>> {
        if self.frozen {
            return None;
        }
        if layout.size() == 0 {
            return Some(NonNull::dangling());
        }
        let align = layout.align();
        let size = layout.size();

        if let Some(mut tail_ptr) = self.tail {
            // SAFETY: `tail_ptr` はこのアリーナが所有する有効なチャンク。
            let tail = unsafe { tail_ptr.as_mut() };
            if let Some(aligned) = align_up(tail.used, align)
                && let Some(end) = aligned.checked_add(size)
                && end <= tail.data_cap
            {
                let delta = end - tail.used;
                if !core.charge_perm(delta) {
                    return None;
                }
                tail.used = end;
                // SAFETY: `aligned < end <= tail.data_cap`。
                let ptr = unsafe { chunk_data_ptr(tail_ptr).add(aligned) };
                return NonNull::new(ptr);
            }
        }

        // 新規チャンクを確保する前に論理上限を確認する。
        if !core.charge_perm(size) {
            return None;
        }
        let desired = self.next_block_size.max(size);
        let Some(mut new_chunk) = alloc_chunk(core, size, desired, align) else {
            core.uncharge_perm(size);
            return None;
        };
        self.next_block_size = (self.next_block_size.saturating_mul(2)).min(self.config.max_block);

        // SAFETY: `new_chunk` は確保直後で `used == 0`、`data_cap >= size`。
        unsafe {
            new_chunk.as_mut().used = size;
        }
        if let Some(mut prev_tail) = self.tail {
            // SAFETY: `prev_tail` は有効なチャンク。
            unsafe {
                prev_tail.as_mut().next = Some(new_chunk);
            }
        } else {
            self.head = Some(new_chunk);
        }
        self.tail = Some(new_chunk);

        // SAFETY: `0 < size <= data_cap`。
        let ptr = unsafe { chunk_data_ptr(new_chunk) };
        NonNull::new(ptr)
    }

    /// 永続領域へ値を 1 つ書き込んで不変ポインタを返す。
    pub fn alloc_val<T>(&mut self, core: &mut AllocCore<'_>, val: T) -> Option<NonNull<T>> {
        let layout = Layout::new::<T>();
        let raw = self.alloc_raw(core, layout)?;
        let ptr = raw.cast::<T>();
        if std::mem::size_of::<T>() > 0 {
            // SAFETY: `ptr` は `Layout::new::<T>()` で確保した未初期化領域。
            unsafe { ptr.as_ptr().write(val) };
        }
        Some(ptr)
    }

    /// 永続領域へ `Copy` スライスをコピーしてポインタと長さを返す。
    pub fn alloc_copy_slice<T: Copy>(
        &mut self,
        core: &mut AllocCore<'_>,
        src: &[T],
    ) -> Option<NonNull<T>> {
        if src.is_empty() || std::mem::size_of::<T>() == 0 {
            return Some(NonNull::dangling());
        }
        let layout = Layout::array::<T>(src.len()).ok()?;
        let raw = self.alloc_raw(core, layout)?;
        let ptr = raw.cast::<T>();
        // SAFETY: `ptr` は `src.len()` 要素分確保されており非重複。
        unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), ptr.as_ptr(), src.len()) };
        Some(ptr)
    }

    /// 確保した全チャンクを `Allocator` へ返却する。
    pub fn destroy(&mut self, core: &mut AllocCore<'_>) {
        let head = self.head.take();
        self.tail = None;
        // SAFETY: `head` 配下はすべて `core` から確保したチャンク。
        unsafe { destroy_chunk_chain(core, head) };
    }
}

/// [`WorkArena`] の巻き戻し位置を表すチェックポイント。
#[derive(Clone, Copy)]
pub(crate) struct WorkMark {
    chunk: Option<NonNull<ChunkHeader>>,
    used: usize,
    total_work_bytes: usize,
}

/// 1 回の評価または読み込みの間に使う作業領域。
///
/// 終了時に `reset` で先頭へ巻き戻し、確保済みチャンクを解放せず再利用する。
pub(crate) struct WorkArena {
    head: Option<NonNull<ChunkHeader>>,
    cur: Option<NonNull<ChunkHeader>>,
    config: RegionConfig,
    next_block_size: usize,
}

impl WorkArena {
    pub fn new(config: RegionConfig) -> Self {
        let config = config.normalized();
        Self {
            head: None,
            cur: None,
            config,
            next_block_size: config.initial_block,
        }
    }

    /// 現在の位置を記録する。後で [`Self::rewind`] に渡して巻き戻せる。
    pub fn mark(&self, core: &AllocCore<'_>) -> WorkMark {
        let used = self.cur.map_or(0, |ptr| {
            // SAFETY: `cur` はこのアリーナが所有する有効なチャンク。
            unsafe { ptr.as_ref().used }
        });
        WorkMark {
            chunk: self.cur,
            used,
            total_work_bytes: core.work_bytes(),
        }
    }

    /// 記録したチェックポイントまで作業領域を巻き戻す。チャンクは再利用のため保持する。
    pub fn rewind(&mut self, core: &mut AllocCore<'_>, mark: WorkMark) {
        match mark.chunk {
            None => {
                self.reset(core);
            }
            Some(mut target) => {
                // SAFETY: `target` はこのアリーナのチャンクチェーン上にある。
                let mut next = unsafe {
                    target.as_mut().used = mark.used;
                    target.as_ref().next
                };
                while let Some(mut c) = next {
                    // SAFETY: チェーン上の後続チャンクを空にリセットする。
                    next = unsafe {
                        c.as_mut().used = 0;
                        c.as_ref().next
                    };
                }
                self.cur = Some(target);
                core.set_work_bytes(mark.total_work_bytes);
            }
        }
    }

    /// 作業領域全体を先頭へ巻き戻す。チャンクは解放せず次回評価で再利用する。
    pub fn reset(&mut self, core: &mut AllocCore<'_>) {
        let mut cursor = self.head;
        while let Some(mut c) = cursor {
            // SAFETY: チェーン上の全チャンクの `used` を 0 に戻す。
            cursor = unsafe {
                c.as_mut().used = 0;
                c.as_ref().next
            };
        }
        self.cur = self.head;
        core.set_work_bytes(0);
    }

    /// 作業領域から `layout` の生領域を確保する。
    pub fn alloc_raw(&mut self, core: &mut AllocCore<'_>, layout: Layout) -> Option<NonNull<u8>> {
        if layout.size() == 0 {
            return Some(NonNull::dangling());
        }
        let align = layout.align();
        let size = layout.size();

        // 現在のチャンクおよび既存の未使用後続チャンクから探す。
        let mut cursor = self.cur;
        let mut last = self.cur;
        while let Some(mut chunk_ptr) = cursor {
            // SAFETY: `chunk_ptr` はこのアリーナが所有する有効なチャンク。
            let chunk = unsafe { chunk_ptr.as_mut() };
            if let Some(aligned) = align_up(chunk.used, align)
                && let Some(end) = aligned.checked_add(size)
                && end <= chunk.data_cap
            {
                let delta = end - chunk.used;
                if !core.charge_work(delta) {
                    return None;
                }
                chunk.used = end;
                self.cur = Some(chunk_ptr);
                // SAFETY: `aligned < end <= chunk.data_cap`。
                let ptr = unsafe { chunk_data_ptr(chunk_ptr).add(aligned) };
                return NonNull::new(ptr);
            }
            last = Some(chunk_ptr);
            cursor = chunk.next;
        }

        // 既存チャンクに収まらないので新規チャンクを確保する。
        let prev_work = core.work_bytes();
        if !core.charge_work(size) {
            return None;
        }
        let desired = self.next_block_size.max(size);
        let Some(mut new_chunk) = alloc_chunk(core, size, desired, align) else {
            core.set_work_bytes(prev_work);
            return None;
        };
        self.next_block_size = (self.next_block_size.saturating_mul(2)).min(self.config.max_block);

        // SAFETY: `new_chunk` は確保直後で `used == 0`、`data_cap >= size`。
        unsafe {
            new_chunk.as_mut().used = size;
        }
        if let Some(mut prev) = last {
            // SAFETY: `prev` はチェーン末尾の有効なチャンク。
            unsafe {
                prev.as_mut().next = Some(new_chunk);
            }
        } else {
            self.head = Some(new_chunk);
        }
        self.cur = Some(new_chunk);

        // SAFETY: `0 < size <= data_cap`。
        let ptr = unsafe { chunk_data_ptr(new_chunk) };
        NonNull::new(ptr)
    }

    /// 作業領域へ値を 1 つ書き込んでポインタを返す。
    pub fn alloc_val<T>(&mut self, core: &mut AllocCore<'_>, val: T) -> Option<NonNull<T>> {
        let layout = Layout::new::<T>();
        let raw = self.alloc_raw(core, layout)?;
        let ptr = raw.cast::<T>();
        if std::mem::size_of::<T>() > 0 {
            // SAFETY: `ptr` は `Layout::new::<T>()` で確保した未初期化領域。
            unsafe { ptr.as_ptr().write(val) };
        }
        Some(ptr)
    }

    /// `ptr` が現在チャンクの末尾直前にある場合、インプレースで `old_size` → `new_size` へ拡張する。
    ///
    /// # Safety
    /// `ptr` はこの `WorkArena` から `old_size` バイトで確保された有効なポインタでなければならない。
    pub unsafe fn try_grow_in_place(
        &mut self,
        core: &mut AllocCore<'_>,
        ptr: NonNull<u8>,
        old_size: usize,
        new_size: usize,
    ) -> bool {
        if new_size <= old_size {
            return true;
        }
        let Some(mut cur_ptr) = self.cur else {
            return false;
        };
        // SAFETY: `cur_ptr` はこのアリーナが所有する有効なチャンク。
        let chunk = unsafe { cur_ptr.as_mut() };
        let data_base = unsafe { chunk_data_ptr(cur_ptr) } as usize;
        let p = ptr.as_ptr() as usize;
        if p < data_base || p.saturating_add(old_size) != data_base + chunk.used {
            return false;
        }
        let delta = new_size - old_size;
        let Some(new_used) = chunk.used.checked_add(delta) else {
            return false;
        };
        if new_used > chunk.data_cap {
            return false;
        }
        if !core.charge_work(delta) {
            return false;
        }
        chunk.used = new_used;
        true
    }

    /// 確保した全チャンクを `Allocator` へ返却する。
    pub fn destroy(&mut self, core: &mut AllocCore<'_>) {
        let head = self.head.take();
        self.cur = None;
        core.set_work_bytes(0);
        // SAFETY: `head` 配下はすべて `core` から確保したチャンク。
        unsafe { destroy_chunk_chain(core, head) };
    }
}
