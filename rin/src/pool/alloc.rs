//! プール上の高レベルオブジェクト・コード領域確保ヘルパ。

use std::alloc::Layout;
use std::ptr::NonNull;

use crate::bytecode::FnProtoPtr;
use crate::error::{ErrRecorded, Span};
use crate::mem::{RawStr, RawVec};

use super::{CodeRegion, ModuleExport, Obj, ObjId, ObjSlice, ObjStr, Pool, RVal, StructDescRef};

impl Pool<'_> {
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

    /// オブジェクト領域上に文字列ペイロードを確保する。
    pub(crate) fn alloc_obj_str(&mut self, s: &str, span: Span) -> Result<ObjStr, ErrRecorded> {
        if s.is_empty() {
            return Ok(ObjStr::empty());
        }
        let Ok(len_u32) = u32::try_from(s.len()) else {
            return Err(self.oom_error(span, "memory limit exceeded"));
        };
        let Some(block) = self.obj_heap.alloc(&mut self.core, s.len(), 1) else {
            return Err(self.oom_limit_error(span));
        };
        // SAFETY: `block.actual_size >= s.len()`。
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), block.ptr.as_ptr(), s.len()) };
        let char_len = if s.is_ascii() {
            len_u32
        } else {
            s.chars().count() as u32
        };
        Ok(ObjStr {
            ptr: block.ptr,
            len: len_u32,
            char_len,
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

    pub(super) fn alloc_obj_slice_uninit<T: Copy>(
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

    /// `Obj::Str` を確保して `ObjId` を返す。
    pub fn alloc_str(&mut self, s: &str, span: Span) -> Result<ObjId, ErrRecorded> {
        let os = self.alloc_obj_str(s, span)?;
        self.alloc_obj(Obj::Str(os), span)
    }

    /// `Obj::Ident` を確保して `ObjId` を返す。
    pub fn alloc_ident(&mut self, s: &str, span: Span) -> Result<ObjId, ErrRecorded> {
        let os = self.alloc_obj_str(s, span)?;
        self.alloc_obj(Obj::Ident(os), span)
    }

    /// 既存の `ObjStr` ペイロードを複製して `Obj::Str` を確保する。
    pub fn alloc_str_from_obj_str(
        &mut self,
        src: ObjStr,
        span: Span,
    ) -> Result<ObjId, ErrRecorded> {
        let os = if src.len == 0 {
            ObjStr::empty()
        } else {
            let n = src.len();
            let Some(block) = self.obj_heap.alloc(&mut self.core, n, 1) else {
                return Err(self.oom_limit_error(span));
            };
            // SAFETY: `src` は既存の `ObjHeap` ブロックで `block` と非重複。
            unsafe { std::ptr::copy_nonoverlapping(src.ptr.as_ptr(), block.ptr.as_ptr(), n) };
            ObjStr {
                ptr: block.ptr,
                len: src.len,
                char_len: src.char_len,
                block_size: block.actual_size,
            }
        };
        self.alloc_obj(Obj::Str(os), span)
    }

    /// 既存の `ObjStr` ペイロードを複製して `Obj::Ident` を確保する。
    pub fn alloc_ident_from_obj_str(
        &mut self,
        src: ObjStr,
        span: Span,
    ) -> Result<ObjId, ErrRecorded> {
        let os = if src.len == 0 {
            ObjStr::empty()
        } else {
            let n = src.len();
            let Some(block) = self.obj_heap.alloc(&mut self.core, n, 1) else {
                return Err(self.oom_limit_error(span));
            };
            // SAFETY: `src` は既存の `ObjHeap` ブロックで `block` と非重複。
            unsafe { std::ptr::copy_nonoverlapping(src.ptr.as_ptr(), block.ptr.as_ptr(), n) };
            ObjStr {
                ptr: block.ptr,
                len: src.len,
                char_len: src.char_len,
                block_size: block.actual_size,
            }
        };
        self.alloc_obj(Obj::Ident(os), span)
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
