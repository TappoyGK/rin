//! 外部 `Allocator` 上の領域に配置した UTF-8 文字列ビュー `RawStr`。

use std::alloc::Layout;
use std::ptr::NonNull;

use super::{AllocCore, ObjBlock, ObjHeap, PermArena, WorkArena};

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

    /// 既存の有効な UTF-8 `&str` をコピーなしで借用する。
    ///
    /// # Safety
    /// `s` がこの `RawStr` の使用期間中ずっと有効でなければならない。
    #[inline]
    pub unsafe fn from_borrowed(s: &str) -> Self {
        if s.is_empty() {
            Self::empty()
        } else {
            // SAFETY: 非空 `&str` のポインタは非 NULL。
            Self {
                ptr: unsafe { NonNull::new_unchecked(s.as_ptr().cast_mut()) },
                len: s.len(),
            }
        }
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
