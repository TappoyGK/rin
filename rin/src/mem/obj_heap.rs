//! オブジェクト領域 (`ObjHeap`): サイズ別フリーリストによる再利用可能アロケータ。
//!
//! 解放されたメモリは外部 `Allocator` へ返さずサイズ別フリーリストで再利用するため、
//! `dealloc` が何もしない `ArenaAllocator` でもメモリ使用量が頭打ちになる。

use std::alloc::Layout;
use std::ptr::NonNull;

use super::{AllocCore, RegionConfig, align_up};

/// サイズクラス一覧（16B 〜 64KB の 2 の冪乗）。
const CLASS_SIZES: [usize; 13] = [
    16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
];
const NUM_CLASSES: usize = CLASS_SIZES.len();
const MAX_SMALL_SIZE: usize = CLASS_SIZES[NUM_CLASSES - 1];
const SLAB_ALIGN: usize = 16;

/// フリーリストの空きブロック先頭に埋め込むノード。
#[repr(C)]
struct FreeNode {
    next: Option<NonNull<FreeNode>>,
}

/// スラブ先頭に埋め込むメタデータ。
#[repr(C)]
struct SlabHeader {
    next: Option<NonNull<SlabHeader>>,
    layout: Layout,
}

/// 64KB 超の大ブロック先頭に埋め込むメタデータ。
#[repr(C)]
struct LargeBlockHeader {
    next: Option<NonNull<LargeBlockHeader>>,
    layout: Layout,
    data_size: usize,
    in_use: bool,
}

#[inline]
fn large_header_size() -> usize {
    align_up(std::mem::size_of::<LargeBlockHeader>(), SLAB_ALIGN).expect("large header overflow")
}

#[inline]
fn slab_header_size() -> usize {
    align_up(std::mem::size_of::<SlabHeader>(), SLAB_ALIGN).expect("slab header overflow")
}

#[inline]
fn class_index_for(size: usize, align: usize) -> Option<usize> {
    let needed = size.max(align).max(CLASS_SIZES[0]);
    if needed > MAX_SMALL_SIZE {
        return None;
    }
    let pow2 = needed.next_power_of_two();
    let idx = pow2.trailing_zeros() as usize - 4; // 16 == 1 << 4
    debug_assert!(idx < NUM_CLASSES && CLASS_SIZES[idx] == pow2);
    Some(idx)
}

/// [`ObjHeap`] が割り当てたメモリブロック。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ObjBlock {
    pub ptr: NonNull<u8>,
    /// 実際に確保・課金されたバイト数（サイズクラスまたは大ブロック実サイズ）。
    pub actual_size: usize,
}

/// サイズ別フリーリストによるオブジェクト用ヒープ。
pub(crate) struct ObjHeap {
    free_lists: [Option<NonNull<FreeNode>>; NUM_CLASSES],
    slabs: Option<NonNull<SlabHeader>>,
    large_blocks: Option<NonNull<LargeBlockHeader>>,
    config: RegionConfig,
}

impl ObjHeap {
    pub fn new(config: RegionConfig) -> Self {
        Self {
            free_lists: [None; NUM_CLASSES],
            slabs: None,
            large_blocks: None,
            config: config.normalized(),
        }
    }

    /// `size` バイト・アラインメント `align` のブロックを確保する。
    ///
    /// 成功時は実際に確保されたブロックサイズを `core` の `obj_bytes` へ加算する。
    pub fn alloc(&mut self, core: &mut AllocCore<'_>, size: usize, align: usize) -> Option<ObjBlock> {
        if size == 0 {
            return Some(ObjBlock {
                ptr: NonNull::dangling(),
                actual_size: 0,
            });
        }
        debug_assert!(align.is_power_of_two());

        if let Some(cls) = class_index_for(size, align) {
            let block_size = CLASS_SIZES[cls];
            if !core.charge_obj(block_size) {
                return None;
            }
            if let Some(node) = self.free_lists[cls] {
                // SAFETY: `node` はこのクラスのスラブから切り出された有効な空きブロック。
                self.free_lists[cls] = unsafe { node.as_ref().next };
                return Some(ObjBlock {
                    ptr: node.cast::<u8>(),
                    actual_size: block_size,
                });
            }
            if !self.refill_class(core, cls) {
                core.uncharge_obj(block_size);
                return None;
            }
            let node = self.free_lists[cls].expect("refill_class populated free list");
            // SAFETY: 直前に `refill_class` が構築した空きノード。
            self.free_lists[cls] = unsafe { node.as_ref().next };
            return Some(ObjBlock {
                ptr: node.cast::<u8>(),
                actual_size: block_size,
            });
        }

        self.alloc_large(core, size, align)
    }

    /// `alloc` で確保したブロックをフリーリストへ返却し、`obj_bytes` から減算する。
    ///
    /// # Safety
    /// `block` はこの `ObjHeap` から確保された未解放のブロックでなければならない。
    pub unsafe fn free(&mut self, core: &mut AllocCore<'_>, block: ObjBlock) {
        if block.actual_size == 0 {
            return;
        }
        core.uncharge_obj(block.actual_size);
        if block.actual_size <= MAX_SMALL_SIZE {
            let cls = block.actual_size.trailing_zeros() as usize - 4;
            debug_assert!(cls < NUM_CLASSES && CLASS_SIZES[cls] == block.actual_size);
            let node = block.ptr.cast::<FreeNode>();
            // SAFETY: `block.actual_size >= 16 >= size_of::<FreeNode>()` かつ 16 バイト境界。
            unsafe {
                node.as_ptr().write(FreeNode {
                    next: self.free_lists[cls],
                });
            }
            self.free_lists[cls] = Some(node);
            return;
        }

        // 大ブロックは直前に埋め込んだヘッダを未使用に戻して再利用可能にする。
        let hdr_size = large_header_size();
        // SAFETY: `alloc_large` が `hdr_size` 分だけ後ろのポインタを返している。
        let hdr_ptr = unsafe { block.ptr.as_ptr().sub(hdr_size).cast::<LargeBlockHeader>() };
        // SAFETY: 有効な `LargeBlockHeader`。
        unsafe {
            debug_assert!((*hdr_ptr).in_use);
            (*hdr_ptr).in_use = false;
        }
    }

    fn refill_class(&mut self, core: &mut AllocCore<'_>, cls: usize) -> bool {
        let block_size = CLASS_SIZES[cls];
        let hdr_size = slab_header_size();
        // 最低 1 ブロック、希望は initial_block 以上（または 16 ブロック分、max_block 以下）。
        let desired_blocks = (self.config.initial_block / block_size).clamp(1, 64);
        let desired_data = (desired_blocks * block_size).min(self.config.max_block.max(block_size));
        let slab_align = SLAB_ALIGN.max(block_size.min(64));
        let data_start = align_up(hdr_size, block_size.min(slab_align)).unwrap_or(hdr_size);
        let min_total = data_start + block_size;
        let desired_total = data_start + desired_data;

        let Some((raw, layout)) = core.alloc_chunk_flexible(min_total, desired_total, slab_align)
        else {
            return false;
        };

        let slab_ptr = raw.cast::<SlabHeader>();
        // SAFETY: `raw` は `slab_align` で確保され `layout.size() >= min_total`。
        unsafe {
            slab_ptr.as_ptr().write(SlabHeader {
                next: self.slabs,
                layout,
            });
        }
        self.slabs = Some(slab_ptr);

        let usable = layout.size().saturating_sub(data_start);
        let count = usable / block_size;
        debug_assert!(count >= 1);

        // 後ろから先頭へ繋ぐことで、先頭ブロックから順に払い出されるようにする。
        let mut head = self.free_lists[cls];
        for i in (0..count).rev() {
            // SAFETY: `data_start + i * block_size + block_size <= layout.size()`。
            let ptr = unsafe { raw.as_ptr().add(data_start + i * block_size) };
            let node = NonNull::new(ptr.cast::<FreeNode>()).unwrap();
            // SAFETY: ブロックサイズは 16 以上で `FreeNode` が収まる。
            unsafe {
                node.as_ptr().write(FreeNode { next: head });
            }
            head = Some(node);
        }
        self.free_lists[cls] = head;
        true
    }

    fn alloc_large(&mut self, core: &mut AllocCore<'_>, size: usize, align: usize) -> Option<ObjBlock> {
        let hdr_size = large_header_size();
        let req_align = SLAB_ALIGN.max(align);
        let data_size = align_up(size, req_align)?;

        // 既存の未使用大ブロックから最適サイズ（data_size 以上 2 倍以下）を探す。
        let mut cursor = self.large_blocks;
        let mut best: Option<NonNull<LargeBlockHeader>> = None;
        let mut best_size = usize::MAX;
        while let Some(hdr_ptr) = cursor {
            // SAFETY: `hdr_ptr` はこの `ObjHeap` が所有する有効な大ブロック。
            let hdr = unsafe { hdr_ptr.as_ref() };
            if !hdr.in_use
                && hdr.data_size >= data_size
                && hdr.data_size <= data_size.saturating_mul(2)
                && hdr.layout.align() >= req_align
                && hdr.data_size < best_size
            {
                best = Some(hdr_ptr);
                best_size = hdr.data_size;
            }
            cursor = hdr.next;
        }
        if let Some(mut hdr_ptr) = best {
            if !core.charge_obj(best_size) {
                return None;
            }
            // SAFETY: `hdr_ptr` は有効な未使用大ブロック。
            unsafe {
                hdr_ptr.as_mut().in_use = true;
                let data_ptr = hdr_ptr.as_ptr().cast::<u8>().add(hdr_size);
                return Some(ObjBlock {
                    ptr: NonNull::new_unchecked(data_ptr),
                    actual_size: best_size,
                });
            }
        }

        if !core.charge_obj(data_size) {
            return None;
        }
        let total = hdr_size.checked_add(data_size)?;
        let layout = Layout::from_size_align(total, req_align).ok()?;
        let Some(raw) = core.alloc_raw(layout) else {
            core.uncharge_obj(data_size);
            return None;
        };
        let hdr_ptr = raw.cast::<LargeBlockHeader>();
        // SAFETY: `raw` は `layout` で確保されており `total >= hdr_size + data_size`。
        unsafe {
            hdr_ptr.as_ptr().write(LargeBlockHeader {
                next: self.large_blocks,
                layout,
                data_size,
                in_use: true,
            });
            self.large_blocks = Some(hdr_ptr);
            let data_ptr = raw.as_ptr().add(hdr_size);
            Some(ObjBlock {
                ptr: NonNull::new_unchecked(data_ptr),
                actual_size: data_size,
            })
        }
    }

    /// 確保した全スラブ・大ブロックを `Allocator` へ返却する。
    pub fn destroy(&mut self, core: &mut AllocCore<'_>) {
        self.free_lists = [None; NUM_CLASSES];
        let mut slab = self.slabs.take();
        while let Some(s) = slab {
            // SAFETY: `s` は `refill_class` で確保された有効なスラブ。
            let (next, layout) = unsafe {
                let r = s.as_ref();
                (r.next, r.layout)
            };
            // SAFETY: `s` は `core.alloc_raw(layout)` 由来。
            unsafe { core.dealloc_raw(s.cast::<u8>(), layout) };
            slab = next;
        }
        let mut large = self.large_blocks.take();
        while let Some(b) = large {
            // SAFETY: `b` は `alloc_large` で確保された有効な大ブロック。
            let (next, layout) = unsafe {
                let r = b.as_ref();
                (r.next, r.layout)
            };
            // SAFETY: `b` は `core.alloc_raw(layout)` 由来。
            unsafe { core.dealloc_raw(b.cast::<u8>(), layout) };
            large = next;
        }
    }
}
