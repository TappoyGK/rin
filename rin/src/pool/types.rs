//! オブジェクトプールで扱う実行時値・オブジェクト型定義。

use std::ptr::NonNull;

use crate::bytecode::{BuiltinId, FnProtoPtr};
use crate::mem::ObjBlock;

use super::Pool;

/// コンパイル出力（`Chunk`・`FnProto`・`NameTable` 等）の配置先領域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeRegion {
    /// 読み込み段階：永続領域 (`PermArena`) に配置する。
    Perm,
    /// 凍結後の評価：作業領域 (`WorkArena`) に配置し、評価終了時に一括巻き戻しする。
    Work,
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
                Obj::Str(_) => "text",
                Obj::Ident(_) => "ident",
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
    pub(super) ptr: NonNull<T>,
    pub(super) len: usize,
    pub(super) block_size: usize,
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
    pub(super) fn block(self) -> ObjBlock {
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

/// オブジェクト領域 (`ObjHeap`) または永続領域 (`PermArena`) 上に確保された UTF-8 文字列。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ObjStr {
    pub(super) ptr: NonNull<u8>,
    pub(super) len: usize,
    pub(super) char_len: usize,
    pub(super) block_size: usize,
}

impl ObjStr {
    pub const fn empty() -> Self {
        Self {
            ptr: NonNull::dangling(),
            len: 0,
            char_len: 0,
            block_size: 0,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn char_len(&self) -> usize {
        self.char_len
    }

    #[inline]
    pub fn is_ascii(&self) -> bool {
        self.char_len == self.len
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

    /// スカラー値単位の添字で `char` を取得する。
    /// 返り値の 2 要素目は非 ASCII 時の走査スカラー値数（ASCII 時は 0）。
    pub fn char_at(&self, index: usize) -> Option<(char, usize)> {
        if index >= self.char_len {
            return None;
        }
        let s = self.as_str();
        if self.is_ascii() {
            return Some((s.as_bytes()[index] as char, 0));
        }
        let off = nth_char_byte_offset(s.as_bytes(), index);
        s[off..].chars().next().map(|c| (c, index + 1))
    }

    /// スカラー値単位の範囲 `start..end` に対応する部分文字列スライスを返す。
    pub fn slice_range(&self, start: usize, end: usize) -> Option<&str> {
        if start > end || end > self.char_len {
            return None;
        }
        if start == end {
            return Some("");
        }
        let s = self.as_str();
        if self.is_ascii() {
            return Some(&s[start..end]);
        }
        let bytes = s.as_bytes();
        let byte_start = nth_char_byte_offset(bytes, start);
        let byte_end = if end == self.char_len {
            bytes.len()
        } else {
            byte_start + nth_char_byte_offset(&bytes[byte_start..], end - start)
        };
        Some(&s[byte_start..byte_end])
    }

    #[inline]
    pub(super) fn block(self) -> ObjBlock {
        ObjBlock {
            ptr: self.ptr,
            actual_size: self.block_size,
        }
    }
}

/// 有効な UTF-8 バイト列の先頭から `n` 番目の Unicode スカラー値のバイト位置を返す。
#[inline]
fn nth_char_byte_offset(bytes: &[u8], n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut count = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        // UTF-8 の先頭バイトは `0b10xxxxxx` 以外（符号付きで -64 以上）。
        if (b as i8) >= -64 {
            if count == n {
                return i;
            }
            count += 1;
        }
    }
    bytes.len()
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

pub(crate) enum Obj {
    Cons {
        head: RVal,
        tail: RVal,
        len: usize,
    },
    Array(ObjSlice<RVal>),
    Str(ObjStr),
    Ident(ObjStr),
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
