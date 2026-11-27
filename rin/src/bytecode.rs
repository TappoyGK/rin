//! バイトコード VM の命令列、定数チャンク、関数プロトタイプ、シンボルテーブルを定義する。

use std::ptr::NonNull;

use crate::ast::Pattern;
use crate::error::Span;
use crate::mem::{RawMap, RawStr, RawVec, fnv1a_bytes};
use crate::pool::{CodeRegion, Pool};

mod sym;

pub(crate) use sym::{BuiltinId, PI_SYM_ID, STATIC_SYM_COUNT, SymTable, static_sym_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    Local(u32),
    Upvalue(u32),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StructDesc {
    pub name: RawStr,
    pub fields: RawVec<RawStr>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FnProto {
    pub name: Option<RawStr>,
    pub module: Option<u32>,
    pub arity: u32,
    pub local_count: u32,
    pub captures: RawVec<Capture>,
    pub chunk: Chunk,
}

/// `PermArena` または `WorkArena` 上に配置された `FnProto` へのコピー可能な不変ポインタ。
#[derive(Clone, Copy)]
pub(crate) struct FnProtoPtr(pub(crate) NonNull<FnProto>);

impl FnProtoPtr {
    #[inline(always)]
    pub(crate) const fn new(ptr: NonNull<FnProto>) -> Self {
        Self(ptr)
    }

    #[inline(always)]
    pub(crate) fn as_ref(&self) -> &FnProto {
        // SAFETY: `FnProto` は対応する `PermArena` / `WorkArena` が生存する間、不変かつ有効。
        unsafe { self.0.as_ref() }
    }
}

impl std::ops::Deref for FnProtoPtr {
    type Target = FnProto;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl std::fmt::Debug for FnProtoPtr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_ref().fmt(f)
    }
}

/// 名前と親ノードの添字。平坦な配列で共有し、解放も再帰させない。
#[derive(Debug, Clone, Copy)]
pub(crate) struct NameNode {
    pub start: usize,
    pub end: usize,
    pub parent: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct NameSite {
    pub ip: usize,
    pub head: usize,
    pub previous: Option<usize>,
}

/// プログラム全体で文字列・ノード・参照位置を共有する。
/// バッファは実際に記録する情報に応じて伸長する。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NameTable {
    pub text: RawVec<u8>,
    pub nodes: RawVec<NameNode>,
    pub sites: RawVec<NameSite>,
}

impl NameTable {
    #[inline]
    pub fn text_str(&self) -> &str {
        // SAFETY: `text` には有効な UTF-8 文字列バイト列のみ追記する。
        unsafe { std::str::from_utf8_unchecked(self.text.as_slice()) }
    }
}

/// チャンクは共有表へのポインタと最後の参照位置の添字だけを持つ。
#[derive(Debug, Clone, Copy)]
pub(crate) struct NameInfo {
    pub table: NonNull<NameTable>,
    pub last_site: usize,
}

impl NameInfo {
    #[inline(always)]
    pub fn table_ref(&self) -> &NameTable {
        // SAFETY: `table` はコンパイル完了後に不変となり、アリーナ生存中は有効。
        unsafe { self.table.as_ref() }
    }
}

struct VisibleNames<'a> {
    text: &'a str,
    nodes: &'a [NameNode],
    head: Option<usize>,
}

impl<'a> Iterator for VisibleNames<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let node = &self.nodes[self.head?];
        self.head = node.parent;
        Some(&self.text[node.start..node.end])
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Chunk {
    pub local_count: u32,
    pub code: RawVec<Op>,
    pub spans: RawVec<Span>,
    pub tick_spans: RawVec<Span>,
    pub strings: RawVec<RawStr>,
    pub str_objs: RawVec<crate::pool::ObjId>,
    pub floats: RawVec<f64>,
    pub structs: RawVec<StructDesc>,
    pub patterns: RawVec<Pattern>,
    pub protos: RawVec<FnProtoPtr>,
    pub export_lists: RawVec<RawVec<RawStr>>,
    pub splice_masks: RawVec<RawVec<u8>>,
    pub names: Option<NameInfo>,
}

impl Chunk {
    pub fn visible_names(&self, ip: usize) -> impl Iterator<Item = &str> {
        let (text, nodes, head) = self.names.as_ref().map_or(("", &[][..], None), |names| {
            let table = names.table_ref();
            let mut site = Some(names.last_site);
            let mut head = None;
            while let Some(index) = site {
                let entry = &table.sites[index];
                if entry.ip == ip {
                    head = Some(entry.head);
                    break;
                }
                if entry.ip < ip {
                    break;
                }
                site = entry.previous;
            }
            (table.text_str(), table.nodes.as_slice(), head)
        });
        VisibleNames { text, nodes, head }
    }

    pub fn emit(
        &mut self,
        pool: &mut Pool<'_>,
        region: CodeRegion,
        op: Op,
        span: Span,
    ) -> Option<usize> {
        let idx = self.code.len();
        pool.push_code(region, &mut self.code, op)?;
        if pool.push_code(region, &mut self.spans, span).is_none() {
            self.code.pop();
            return None;
        }
        Some(idx)
    }

    pub fn add_string(
        &mut self,
        pool: &mut Pool<'_>,
        region: CodeRegion,
        string_map: &mut RawMap<RawStr, u32>,
        s: &str,
    ) -> Option<u32> {
        let hash = fnv1a_bytes(s.as_bytes());
        if !string_map.is_empty() {
            if let Some(idx) = string_map.get_by(hash, |k| k.as_str() == s) {
                return Some(idx);
            }
        } else if let Some(pos) = self.strings.as_slice().iter().position(|x| x.as_str() == s) {
            return Some(pos as u32);
        }
        let idx = self.strings.len() as u32;
        let raw = pool.alloc_code_str(region, s)?;
        pool.push_code(region, &mut self.strings, raw)?;
        if !string_map.is_empty() {
            string_map.insert_work(&mut pool.work, &mut pool.core, hash, raw, idx, |a, b| {
                a.as_str() == b.as_str()
            })?;
        } else if self.strings.len() > 16 {
            for (i, &st) in self.strings.as_slice().iter().enumerate() {
                let h = fnv1a_bytes(st.as_str().as_bytes());
                string_map.insert_work(
                    &mut pool.work,
                    &mut pool.core,
                    h,
                    st,
                    i as u32,
                    |a, b| a.as_str() == b.as_str(),
                )?;
            }
        }
        Some(idx)
    }

    pub fn add_float(&mut self, pool: &mut Pool<'_>, region: CodeRegion, x: f64) -> Option<u32> {
        let idx = self.floats.len() as u32;
        pool.push_code(region, &mut self.floats, x)?;
        Some(idx)
    }

    pub fn add_struct(
        &mut self,
        pool: &mut Pool<'_>,
        region: CodeRegion,
        desc: StructDesc,
    ) -> Option<u32> {
        let idx = self.structs.len() as u32;
        pool.push_code(region, &mut self.structs, desc)?;
        Some(idx)
    }

    pub fn add_pattern(
        &mut self,
        pool: &mut Pool<'_>,
        region: CodeRegion,
        pat: Pattern,
    ) -> Option<u32> {
        let copied = copy_pattern_to_code(pool, region, pat)?;
        let idx = self.patterns.len() as u32;
        pool.push_code(region, &mut self.patterns, copied)?;
        Some(idx)
    }

    pub fn add_proto(
        &mut self,
        pool: &mut Pool<'_>,
        region: CodeRegion,
        proto: FnProtoPtr,
    ) -> Option<u32> {
        let idx = self.protos.len() as u32;
        pool.push_code(region, &mut self.protos, proto)?;
        Some(idx)
    }

    pub fn add_exports(
        &mut self,
        pool: &mut Pool<'_>,
        region: CodeRegion,
        names: RawVec<RawStr>,
    ) -> Option<u32> {
        let idx = self.export_lists.len() as u32;
        pool.push_code(region, &mut self.export_lists, names)?;
        Some(idx)
    }

    pub fn add_splice_mask(
        &mut self,
        pool: &mut Pool<'_>,
        region: CodeRegion,
        mask: RawVec<u8>,
    ) -> Option<u32> {
        let idx = self.splice_masks.len() as u32;
        pool.push_code(region, &mut self.splice_masks, mask)?;
        Some(idx)
    }

    /// `WorkArena` 上で構築したチャンクを、`PermArena` へ余剰容量ゼロの密なスライスとしてコピーする。
    #[inline(never)]
    pub fn copy_to_perm(&self, pool: &mut Pool<'_>) -> Option<Self> {
        let code =
            RawVec::from_slice_exact_perm(&mut pool.perm, &mut pool.core, self.code.as_slice())?;
        let spans =
            RawVec::from_slice_exact_perm(&mut pool.perm, &mut pool.core, self.spans.as_slice())?;
        let tick_spans = RawVec::from_slice_exact_perm(
            &mut pool.perm,
            &mut pool.core,
            self.tick_spans.as_slice(),
        )?;
        let floats =
            RawVec::from_slice_exact_perm(&mut pool.perm, &mut pool.core, self.floats.as_slice())?;
        let protos =
            RawVec::from_slice_exact_perm(&mut pool.perm, &mut pool.core, self.protos.as_slice())?;

        let (strings, str_objs) = if self.strings.is_empty() {
            (RawVec::new(), RawVec::new())
        } else {
            let mut v = RawVec::new();
            let mut objs = RawVec::new();
            v.reserve_perm(&mut pool.perm, &mut pool.core, self.strings.len())?;
            objs.reserve_perm(&mut pool.perm, &mut pool.core, self.strings.len())?;
            for &s in self.strings.as_slice() {
                let raw = RawStr::from_perm(&mut pool.perm, &mut pool.core, s.as_str())?;
                v.push_perm(&mut pool.perm, &mut pool.core, raw)?;
                let id = pool.intern_perm_str(raw)?;
                objs.push_perm(&mut pool.perm, &mut pool.core, id)?;
            }
            (v, objs)
        };

        let structs = if self.structs.is_empty() {
            RawVec::new()
        } else {
            let mut v = RawVec::new();
            v.reserve_perm(&mut pool.perm, &mut pool.core, self.structs.len())?;
            for &desc in self.structs.as_slice() {
                let name = RawStr::from_perm(&mut pool.perm, &mut pool.core, desc.name.as_str())?;
                let mut fields = RawVec::new();
                if !desc.fields.is_empty() {
                    fields.reserve_perm(&mut pool.perm, &mut pool.core, desc.fields.len())?;
                    for &f in desc.fields.as_slice() {
                        let fraw = RawStr::from_perm(&mut pool.perm, &mut pool.core, f.as_str())?;
                        fields.push_perm(&mut pool.perm, &mut pool.core, fraw)?;
                    }
                }
                v.push_perm(&mut pool.perm, &mut pool.core, StructDesc { name, fields })?;
            }
            v
        };

        let patterns = if self.patterns.is_empty() {
            RawVec::new()
        } else {
            let mut v = RawVec::new();
            v.reserve_perm(&mut pool.perm, &mut pool.core, self.patterns.len())?;
            for &pat in self.patterns.as_slice() {
                let copied = copy_pattern_to_code(pool, CodeRegion::Perm, pat)?;
                v.push_perm(&mut pool.perm, &mut pool.core, copied)?;
            }
            v
        };

        let export_lists = if self.export_lists.is_empty() {
            RawVec::new()
        } else {
            let mut v = RawVec::new();
            v.reserve_perm(&mut pool.perm, &mut pool.core, self.export_lists.len())?;
            for &list in self.export_lists.as_slice() {
                let mut inner = RawVec::new();
                if !list.is_empty() {
                    inner.reserve_perm(&mut pool.perm, &mut pool.core, list.len())?;
                    for &s in list.as_slice() {
                        let raw = RawStr::from_perm(&mut pool.perm, &mut pool.core, s.as_str())?;
                        inner.push_perm(&mut pool.perm, &mut pool.core, raw)?;
                    }
                }
                v.push_perm(&mut pool.perm, &mut pool.core, inner)?;
            }
            v
        };

        let splice_masks = if self.splice_masks.is_empty() {
            RawVec::new()
        } else {
            let mut v = RawVec::new();
            v.reserve_perm(&mut pool.perm, &mut pool.core, self.splice_masks.len())?;
            for &mask in self.splice_masks.as_slice() {
                let inner =
                    RawVec::from_slice_exact_perm(&mut pool.perm, &mut pool.core, mask.as_slice())?;
                v.push_perm(&mut pool.perm, &mut pool.core, inner)?;
            }
            v
        };

        Some(Self {
            local_count: self.local_count,
            code,
            spans,
            tick_spans,
            strings,
            str_objs,
            floats,
            structs,
            patterns,
            protos,
            export_lists,
            splice_masks,
            names: self.names,
        })
    }
}

fn copy_opt_str(
    pool: &mut Pool<'_>,
    region: CodeRegion,
    s: Option<RawStr>,
) -> Option<Option<RawStr>> {
    match s {
        None => Some(None),
        Some(raw) => Some(Some(pool.alloc_code_str(region, raw.as_str())?)),
    }
}

fn copy_pattern_to_code(pool: &mut Pool<'_>, region: CodeRegion, pat: Pattern) -> Option<Pattern> {
    if region == CodeRegion::Work {
        return Some(pat);
    }
    Some(match pat {
        Pattern::Wildcard => Pattern::Wildcard,
        Pattern::Int(n) => Pattern::Int(n),
        Pattern::Bool(b) => Pattern::Bool(b),
        Pattern::Char(c) => Pattern::Char(c),
        Pattern::Str(s) => Pattern::Str(pool.alloc_code_str(region, s.as_str())?),
        Pattern::Ident(s) => Pattern::Ident(pool.alloc_code_str(region, s.as_str())?),
        Pattern::None => Pattern::None,
        Pattern::Some(b) => Pattern::Some(copy_opt_str(pool, region, b)?),
        Pattern::Ok(b) => Pattern::Ok(copy_opt_str(pool, region, b)?),
        Pattern::Err(b) => Pattern::Err(copy_opt_str(pool, region, b)?),
        Pattern::Variant {
            type_name,
            variant,
            binding,
        } => Pattern::Variant {
            type_name: pool.alloc_code_str(region, type_name.as_str())?,
            variant: pool.alloc_code_str(region, variant.as_str())?,
            binding: copy_opt_str(pool, region, binding)?,
        },
        Pattern::Var(s) => Pattern::Var(pool.alloc_code_str(region, s.as_str())?),
    })
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Op {
    Tick(u32, u32), // (tick_spans 開始添字, 式数)
    Void,
    Int(i64),
    Float(u32),
    Bool(bool),
    Char(char),
    Str(u32),
    Ident(u32),
    None,
    EmptyList,

    GetLocal(u32),
    SetLocal(u32),
    GetCapture(u32),
    GetGlobal(u32),
    DefGlobal(u32),
    DefFn(u32, u32), // (名前シンボルID, プロトタイプ添字)

    MakeList(u32),
    MakeListSplice(u32),
    FormatTemplate(u32),
    MakeTuple(u32),
    MakeStruct(u32),
    WithCheckStruct,
    WithCheckField(u32, u32),
    WithFinish(u32),
    MakeVariant {
        type_name: u32,
        variant: u32,
        has_data: bool,
    },
    MakeSome,
    MakeOk,
    MakeErr,

    GetField(u32),
    GetIndex,
    MakeClosure(u32),

    Check(bool), // true: check_eq, false: check
    Call(u32),
    TailCall(u32),
    And,
    Or,
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfFalsePeek(u32),
    JumpIfTruePeek(u32),
    Pop,

    TryMatch {
        pat_idx: u32,
        fail_ip: u32,
    },
    MatchFail,

    Import(u32),
    Export(u32),
}
