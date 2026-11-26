//! バイトコード VM の軽量フレーム表現 (`RinFrame`, `FrameEntry`, `FrameAction`)。

use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::ptr::NonNull;

use crate::bytecode::{BuiltinId, Chunk, FnProto, FnProtoPtr};
use crate::error::{ErrRecorded, Span};
use crate::mem::WorkBox;
use crate::pool::{ObjSlice, RVal};

use super::super::builtins::callbacks::BuiltinFrame;
use super::super::{CallOrigin, FrameSource};

/// タグ付きポインタで `FrameSource<'a>` を 1 ワード（8 バイト）へ圧縮して格納する。
/// ビット 0 が 0 なら `&Chunk`、1 なら `FnProtoPtr`。
#[inline(always)]
fn encode_source(src: FrameSource<'_>) -> NonZeroUsize {
    let raw = match src {
        FrameSource::Top(c) => c as *const Chunk as usize,
        FrameSource::Proto(p) => (p.0.as_ptr() as usize) | 1,
    };
    // SAFETY: `&Chunk`・`FnProtoPtr` ともに非 NULL。
    unsafe { NonZeroUsize::new_unchecked(raw) }
}

#[inline(always)]
unsafe fn decode_source<'a>(raw: usize) -> FrameSource<'a> {
    if (raw & 1) == 0 {
        // SAFETY: `encode_source` で格納した `&'a Chunk` ポインタ。
        FrameSource::Top(unsafe { &*(raw as *const Chunk) })
    } else {
        // SAFETY: `encode_source` で格納した非 NULL の `FnProtoPtr`。
        FrameSource::Proto(FnProtoPtr(unsafe {
            NonNull::new_unchecked((raw & !1) as *mut FnProto)
        }))
    }
}

/// 作業領域の呼び出しフレームスタックに積む軽量 Rin フレーム（厳密 64 バイト・1 キャッシュライン）。
/// `entry` が `NonZeroUsize` のため `FrameEntry` も 64 バイトに収まる。
#[derive(Clone, Copy)]
pub(in super::super) struct RinFrame<'a> {
    entry: NonZeroUsize,
    tail_source: usize,
    pub(super) captures: *const RVal,
    pub(super) tail_calls: u32,
    pub(super) base: u32,
    pub(super) frame_stack_start: u32,
    pub(super) ip: u32,
    pub(super) cleanup_stack_to: u32,
    origin_line: u32,
    origin_col: u32,
    caller_module: u32,
    prev_active_module: u32,
    origin_kind: u8,
    origin_builtin: u8,
    pub(super) counted_depth: bool,
    _marker: PhantomData<&'a Chunk>,
}

const NO_MODULE: u32 = u32::MAX;
const ORIGIN_NONE: u8 = 0;
const ORIGIN_RIN: u8 = 1;
const ORIGIN_HOST: u8 = 2;
const ORIGIN_BUILTIN: u8 = 3;

const _: () = assert!(BuiltinId::ALL.len() <= u8::MAX as usize);

impl<'a> RinFrame<'a> {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(in super::super) fn new(
        entry: FrameSource<'a>,
        captures: ObjSlice<RVal>,
        base: usize,
        frame_stack_start: usize,
        origin: Option<CallOrigin>,
        caller_module: Option<u32>,
        prev_active_module: Option<u32>,
        cleanup_stack_to: usize,
        counted_depth: bool,
    ) -> Self {
        debug_assert!(base <= u32::MAX as usize);
        debug_assert!(frame_stack_start <= u32::MAX as usize);
        debug_assert!(cleanup_stack_to <= u32::MAX as usize);
        debug_assert!(caller_module != Some(NO_MODULE));
        debug_assert!(prev_active_module != Some(NO_MODULE));
        let (origin_kind, origin_line, origin_col, origin_builtin) = match origin {
            None => (ORIGIN_NONE, 0, 0, 0),
            Some(CallOrigin::Rin(s)) => (
                ORIGIN_RIN,
                s.line.min(u32::MAX as usize) as u32,
                s.col.min(u32::MAX as usize) as u32,
                0,
            ),
            Some(CallOrigin::Host) => (ORIGIN_HOST, 0, 0, 0),
            Some(CallOrigin::Builtin { id, at }) => (
                ORIGIN_BUILTIN,
                at.line.min(u32::MAX as usize) as u32,
                at.col.min(u32::MAX as usize) as u32,
                id as u8,
            ),
        };
        Self {
            entry: encode_source(entry),
            tail_source: 0,
            captures: captures.as_slice().as_ptr(),
            tail_calls: 0,
            base: base as u32,
            frame_stack_start: frame_stack_start as u32,
            ip: 0,
            cleanup_stack_to: cleanup_stack_to as u32,
            origin_line,
            origin_col,
            caller_module: caller_module.unwrap_or(NO_MODULE),
            prev_active_module: prev_active_module.unwrap_or(NO_MODULE),
            origin_kind,
            origin_builtin,
            counted_depth,
            _marker: PhantomData,
        }
    }

    #[inline(always)]
    pub(super) fn entry_source(&self) -> FrameSource<'a> {
        // SAFETY: `self.entry` は `encode_source` で格納済み。
        unsafe { decode_source(self.entry.get()) }
    }

    #[inline(always)]
    pub(super) fn tail_source(&self) -> Option<FrameSource<'a>> {
        if self.tail_source == 0 {
            None
        } else {
            // SAFETY: 非 0 の `self.tail_source` は `encode_source` で格納済み。
            Some(unsafe { decode_source(self.tail_source) })
        }
    }

    #[inline(always)]
    pub(super) fn has_tail(&self) -> bool {
        self.tail_source != 0
    }

    #[inline(always)]
    pub(super) fn set_tail_proto(&mut self, proto: FnProtoPtr) {
        self.tail_source = encode_source(FrameSource::Proto(proto)).get();
    }

    #[inline(always)]
    pub(super) fn active_source(&self) -> FrameSource<'a> {
        let raw = if self.tail_source != 0 {
            self.tail_source
        } else {
            self.entry.get()
        };
        // SAFETY: `raw` は `encode_source` で格納済み。
        unsafe { decode_source(raw) }
    }

    #[inline(always)]
    pub(super) fn active_chunk(&self) -> &'a Chunk {
        let raw = if self.tail_source != 0 {
            self.tail_source
        } else {
            self.entry.get()
        };
        if (raw & 1) == 0 {
            // SAFETY: `encode_source` で格納した `&'a Chunk`。
            unsafe { &*(raw as *const Chunk) }
        } else {
            // SAFETY: `encode_source` で格納した `FnProtoPtr`。
            unsafe { &(*((raw & !1) as *const FnProto)).chunk }
        }
    }

    #[inline(always)]
    pub(super) fn prev_mod(&self) -> Option<u32> {
        (self.prev_active_module != NO_MODULE).then_some(self.prev_active_module)
    }

    #[inline(always)]
    pub(super) fn caller_mod(&self) -> Option<u32> {
        (self.caller_module != NO_MODULE).then_some(self.caller_module)
    }

    #[inline(always)]
    pub(super) fn origin(&self) -> Option<CallOrigin> {
        match self.origin_kind {
            ORIGIN_RIN => Some(CallOrigin::Rin(Span {
                line: self.origin_line as usize,
                col: self.origin_col as usize,
            })),
            ORIGIN_HOST => Some(CallOrigin::Host),
            ORIGIN_BUILTIN => Some(CallOrigin::Builtin {
                id: BuiltinId::ALL[self.origin_builtin as usize],
                at: Span {
                    line: self.origin_line as usize,
                    col: self.origin_col as usize,
                },
            }),
            _ => None,
        }
    }

    #[inline(always)]
    pub(super) fn capture_at(&self, idx: usize) -> RVal {
        // SAFETY: コンパイラが `idx < proto.captures.len()` を保証しており、
        // `captures` はクロージャ生存中（スタック上または親フレーム保持中）有効。
        unsafe { *self.captures.add(idx) }
    }
}

pub(super) enum FrameEntry<'a> {
    Rin(RinFrame<'a>),
    Builtin(WorkBox<BuiltinFrame>),
}

const _: () = {
    assert!(std::mem::size_of::<RinFrame<'_>>() <= 64);
    assert!(std::mem::size_of::<FrameEntry<'_>>() <= 64);
};

pub(super) enum FrameAction {
    Done(RVal),
    ReturnToBuiltin(RVal),
    FailUnwound(ErrRecorded),
    CallCallbackBuiltin {
        builtin: BuiltinId,
        arg_start: usize,
        argc: usize,
        span: Span,
    },
}
