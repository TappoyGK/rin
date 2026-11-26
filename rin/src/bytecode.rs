//! バイトコード VM の命令列、定数チャンク、関数プロトタイプ、シンボルテーブルを定義する。

use std::cell::{Cell, OnceCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::ast::OwnedPattern as Pattern;
use crate::error::Span;
use crate::mem::{RawMap, RawStr, RawVec, fnv1a_bytes};
use crate::pool::Pool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum BuiltinId {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Not,
    AsF64,
    AsI64,
    Sin,
    Cos,
    Sqrt,
    Abs,
    Floor,
    Ceil,
    List,
    Len,
    IsEmpty,
    Get,
    Car,
    Cdr,
    Last,
    Cons,
    Push,
    Append,
    Flatten,
    Slice,
    Take,
    Drop,
    Reverse,
    Range,
    Contains,
    Find,
    Any,
    All,
    Map,
    Filter,
    FlatMap,
    Fold,
    Zip,
    Enumerate,
    Sort,
    SortBy,
    Unique,
    ListToArray,
    ArrayToList,
}

impl BuiltinId {
    pub const ALL: &[Self] = &[
        Self::Add,
        Self::Sub,
        Self::Mul,
        Self::Div,
        Self::Rem,
        Self::Eq,
        Self::Ne,
        Self::Lt,
        Self::Le,
        Self::Gt,
        Self::Ge,
        Self::And,
        Self::Or,
        Self::Not,
        Self::AsF64,
        Self::AsI64,
        Self::Sin,
        Self::Cos,
        Self::Sqrt,
        Self::Abs,
        Self::Floor,
        Self::Ceil,
        Self::List,
        Self::Len,
        Self::IsEmpty,
        Self::Get,
        Self::Car,
        Self::Cdr,
        Self::Last,
        Self::Cons,
        Self::Push,
        Self::Append,
        Self::Flatten,
        Self::Slice,
        Self::Take,
        Self::Drop,
        Self::Reverse,
        Self::Range,
        Self::Contains,
        Self::Find,
        Self::Any,
        Self::All,
        Self::Map,
        Self::Filter,
        Self::FlatMap,
        Self::Fold,
        Self::Zip,
        Self::Enumerate,
        Self::Sort,
        Self::SortBy,
        Self::Unique,
        Self::ListToArray,
        Self::ArrayToList,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Rem => "%",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::And => "and",
            Self::Or => "or",
            Self::Not => "not",
            Self::AsF64 => "as_f64",
            Self::AsI64 => "as_i64",
            Self::Sin => "sin",
            Self::Cos => "cos",
            Self::Sqrt => "sqrt",
            Self::Abs => "abs",
            Self::Floor => "floor",
            Self::Ceil => "ceil",
            Self::List => "list",
            Self::Len => "len",
            Self::IsEmpty => "is_empty",
            Self::Get => "get",
            Self::Car => "car",
            Self::Cdr => "cdr",
            Self::Last => "last",
            Self::Cons => "cons",
            Self::Push => "push",
            Self::Append => "append",
            Self::Flatten => "flatten",
            Self::Slice => "slice",
            Self::Take => "take",
            Self::Drop => "drop",
            Self::Reverse => "reverse",
            Self::Range => "range",
            Self::Contains => "contains",
            Self::Find => "find",
            Self::Any => "any",
            Self::All => "all",
            Self::Map => "map",
            Self::Filter => "filter",
            Self::FlatMap => "flat_map",
            Self::Fold => "fold",
            Self::Zip => "zip",
            Self::Enumerate => "enumerate",
            Self::Sort => "sort",
            Self::SortBy => "sort_by",
            Self::Unique => "unique",
            Self::ListToArray => "list->array",
            Self::ArrayToList => "array->list",
        }
    }
}

/// 静的シンボル（組み込み関数 53 個 + `pi`）の総数。`Builder::new` でメモリを確保しないために固定 ID を割り当てる。
pub(crate) const PI_SYM_ID: u32 = BuiltinId::ALL.len() as u32;
pub(crate) const STATIC_SYM_COUNT: u32 = PI_SYM_ID + 1;

pub(crate) fn static_sym_id(name: &str) -> Option<u32> {
    match name {
        "+" => Some(BuiltinId::Add as u32),
        "-" => Some(BuiltinId::Sub as u32),
        "*" => Some(BuiltinId::Mul as u32),
        "/" => Some(BuiltinId::Div as u32),
        "%" => Some(BuiltinId::Rem as u32),
        "==" => Some(BuiltinId::Eq as u32),
        "!=" => Some(BuiltinId::Ne as u32),
        "<" => Some(BuiltinId::Lt as u32),
        "<=" => Some(BuiltinId::Le as u32),
        ">" => Some(BuiltinId::Gt as u32),
        ">=" => Some(BuiltinId::Ge as u32),
        "and" => Some(BuiltinId::And as u32),
        "or" => Some(BuiltinId::Or as u32),
        "not" => Some(BuiltinId::Not as u32),
        "as_f64" => Some(BuiltinId::AsF64 as u32),
        "as_i64" => Some(BuiltinId::AsI64 as u32),
        "sin" => Some(BuiltinId::Sin as u32),
        "cos" => Some(BuiltinId::Cos as u32),
        "sqrt" => Some(BuiltinId::Sqrt as u32),
        "abs" => Some(BuiltinId::Abs as u32),
        "floor" => Some(BuiltinId::Floor as u32),
        "ceil" => Some(BuiltinId::Ceil as u32),
        "list" => Some(BuiltinId::List as u32),
        "len" => Some(BuiltinId::Len as u32),
        "is_empty" => Some(BuiltinId::IsEmpty as u32),
        "get" => Some(BuiltinId::Get as u32),
        "car" => Some(BuiltinId::Car as u32),
        "cdr" => Some(BuiltinId::Cdr as u32),
        "last" => Some(BuiltinId::Last as u32),
        "cons" => Some(BuiltinId::Cons as u32),
        "push" => Some(BuiltinId::Push as u32),
        "append" => Some(BuiltinId::Append as u32),
        "flatten" => Some(BuiltinId::Flatten as u32),
        "slice" => Some(BuiltinId::Slice as u32),
        "take" => Some(BuiltinId::Take as u32),
        "drop" => Some(BuiltinId::Drop as u32),
        "reverse" => Some(BuiltinId::Reverse as u32),
        "range" => Some(BuiltinId::Range as u32),
        "contains" => Some(BuiltinId::Contains as u32),
        "find" => Some(BuiltinId::Find as u32),
        "any" => Some(BuiltinId::Any as u32),
        "all" => Some(BuiltinId::All as u32),
        "map" => Some(BuiltinId::Map as u32),
        "filter" => Some(BuiltinId::Filter as u32),
        "flat_map" => Some(BuiltinId::FlatMap as u32),
        "fold" => Some(BuiltinId::Fold as u32),
        "zip" => Some(BuiltinId::Zip as u32),
        "enumerate" => Some(BuiltinId::Enumerate as u32),
        "sort" => Some(BuiltinId::Sort as u32),
        "sort_by" => Some(BuiltinId::SortBy as u32),
        "unique" => Some(BuiltinId::Unique as u32),
        "list->array" => Some(BuiltinId::ListToArray as u32),
        "array->list" => Some(BuiltinId::ArrayToList as u32),
        "pi" => Some(PI_SYM_ID),
        _ => None,
    }
}

pub(crate) fn static_sym_name(id: u32) -> Option<&'static str> {
    let idx = id as usize;
    if idx < BuiltinId::ALL.len() {
        Some(BuiltinId::ALL[idx].name())
    } else if id == PI_SYM_ID {
        Some("pi")
    } else {
        None
    }
}

/// 永続領域 (`PermArena`) と凍結後一時作業領域 (`WorkArena`) の 2 層で構成するシンボル表。
pub(crate) struct SymTable {
    perm_names: RawVec<RawStr>,
    perm_ids: RawMap<RawStr, u32>,
    temp_names: RawVec<RawStr>,
    temp_ids: RawMap<RawStr, u32>,
    // 頻出する永続シンボルの再参照キャッシュ。検査数は常に 4 以下。
    recent: [Option<u32>; 4],
    lookup: Cell<Option<u32>>,
}

impl Default for SymTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SymTable {
    pub const fn new() -> Self {
        Self {
            perm_names: RawVec::new(),
            perm_ids: RawMap::new(),
            temp_names: RawVec::new(),
            temp_ids: RawMap::new(),
            recent: [None; 4],
            lookup: Cell::new(None),
        }
    }

    #[inline]
    pub fn name(&self, id: u32) -> Option<&str> {
        if let Some(s) = static_sym_name(id) {
            return Some(s);
        }
        let dyn_idx = (id - STATIC_SYM_COUNT) as usize;
        if dyn_idx < self.perm_names.len() {
            return Some(self.perm_names.as_slice()[dyn_idx].as_str());
        }
        let temp_idx = dyn_idx - self.perm_names.len();
        self.temp_names.as_slice().get(temp_idx).map(RawStr::as_str)
    }

    pub fn intern(&mut self, pool: &mut Pool<'_>, name: &str) -> Option<u32> {
        if let Some(id) = static_sym_id(name) {
            return Some(id);
        }
        for index in 0..self.recent.len() {
            if let Some(id) = self.recent[index]
                && self.name(id) == Some(name)
            {
                self.recent.swap(0, index);
                return Some(id);
            }
        }
        let hash = fnv1a_bytes(name.as_bytes());
        if let Some(id) = self.perm_ids.get_by(hash, |k| k.as_str() == name) {
            self.recent.rotate_right(1);
            self.recent[0] = Some(id);
            return Some(id);
        }
        if !pool.perm.is_frozen() {
            let raw = RawStr::from_perm(&mut pool.perm, &mut pool.core, name)?;
            let id = STATIC_SYM_COUNT.checked_add(self.perm_names.len() as u32)?;
            self.perm_names.push_perm(&mut pool.perm, &mut pool.core, raw)?;
            if self
                .perm_ids
                .insert_perm(
                    &mut pool.perm,
                    &mut pool.core,
                    hash,
                    raw,
                    id,
                    |a, b| a.as_str() == b.as_str(),
                )
                .is_none()
            {
                self.perm_names.pop();
                return None;
            }
            self.recent.rotate_right(1);
            self.recent[0] = Some(id);
            Some(id)
        } else {
            if let Some(id) = self.temp_ids.get_by(hash, |k| k.as_str() == name) {
                return Some(id);
            }
            let raw = RawStr::from_work(&mut pool.work, &mut pool.core, name)?;
            let total = self.perm_names.len().checked_add(self.temp_names.len())?;
            let id = STATIC_SYM_COUNT.checked_add(u32::try_from(total).ok()?)?;
            self.temp_names.push_work(&mut pool.work, &mut pool.core, raw)?;
            if self
                .temp_ids
                .insert_work(
                    &mut pool.work,
                    &mut pool.core,
                    hash,
                    raw,
                    id,
                    |a, b| a.as_str() == b.as_str(),
                )
                .is_none()
            {
                self.temp_names.pop();
                return None;
            }
            Some(id)
        }
    }

    pub fn get(&self, name: &str) -> Option<u32> {
        if let Some(id) = static_sym_id(name) {
            return Some(id);
        }
        if let Some(id) = self.lookup.get()
            && self.name(id) == Some(name)
        {
            return Some(id);
        }
        let hash = fnv1a_bytes(name.as_bytes());
        let id = self
            .perm_ids
            .get_by(hash, |k| k.as_str() == name)
            .or_else(|| self.temp_ids.get_by(hash, |k| k.as_str() == name));
        self.lookup.set(id);
        id
    }

    /// 作業領域の巻き戻しに合わせて凍結後の一時シンボルを破棄する。
    pub fn clear_temp(&mut self) {
        self.temp_names = RawVec::new();
        self.temp_ids = RawMap::new();
        self.lookup.set(None);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    Local(u32),
    Upvalue(u32),
}

#[derive(Debug, Clone)]
pub(crate) struct StructDesc {
    pub name: Rc<str>,
    pub fields: Vec<Rc<str>>,
}

#[derive(Debug, Clone)]
pub(crate) struct FnProto {
    pub name: Option<String>,
    pub module: Option<u32>,
    pub arity: u32,
    pub local_count: u32,
    pub captures: Vec<Capture>,
    pub chunk: Chunk,
}

/// 名前と親ノードの添字。平坦な配列で共有し、解放も再帰させない。
#[derive(Debug)]
pub(crate) struct NameNode {
    pub start: usize,
    pub end: usize,
    pub parent: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct NameSite {
    pub ip: usize,
    pub head: usize,
    pub previous: Option<usize>,
}

/// プログラム全体で文字列・ノード・参照位置を共有する。
/// バッファは実際に記録する情報に応じて伸長する。
#[derive(Debug, Default)]
pub(crate) struct NameTable {
    pub text: String,
    pub nodes: Vec<NameNode>,
    pub sites: Vec<NameSite>,
}

/// チャンクは共有表への参照と最後の参照位置の添字だけを持つ。
#[derive(Debug, Clone)]
pub(crate) struct NameInfo {
    pub table: Rc<OnceCell<NameTable>>,
    pub last_site: usize,
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

#[derive(Debug, Clone, Default)]
pub(crate) struct Chunk {
    pub local_count: u32,
    pub code: Vec<Op>,
    pub spans: Vec<Span>,
    pub tick_spans: Vec<Span>,
    pub strings: Vec<Rc<str>>,
    string_map: Option<BTreeMap<Rc<str>, u32>>,
    pub floats: Vec<f64>,
    pub structs: Vec<StructDesc>,
    pub patterns: Vec<Pattern>,
    pub protos: Vec<Rc<FnProto>>,
    pub export_lists: Vec<Vec<String>>,
    pub names: Option<NameInfo>,
}

impl Chunk {
    pub fn visible_names(&self, ip: usize) -> impl Iterator<Item = &str> {
        let (text, nodes, head) = self.names.as_ref().map_or(("", &[][..], None), |names| {
            let table = names.table.get().expect("name table finalized");
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
            (table.text.as_str(), table.nodes.as_slice(), head)
        });
        VisibleNames { text, nodes, head }
    }

    pub fn emit(&mut self, op: Op, span: Span) -> usize {
        let idx = self.code.len();
        self.code.push(op);
        self.spans.push(span);
        idx
    }

    pub fn add_string(&mut self, s: &str) -> u32 {
        if let Some(map) = &self.string_map {
            if let Some(&idx) = map.get(s) {
                return idx;
            }
        } else {
            if let Some(pos) = self.strings.iter().position(|x| x.as_ref() == s) {
                return pos as u32;
            }
        }
        let idx = self.strings.len() as u32;
        let rc: Rc<str> = Rc::from(s);
        self.strings.push(Rc::clone(&rc));
        if let Some(map) = &mut self.string_map {
            map.insert(rc, idx);
        } else if self.strings.len() > 16 {
            let mut map = BTreeMap::new();
            for (i, st) in self.strings.iter().enumerate() {
                map.insert(Rc::clone(st), i as u32);
            }
            self.string_map = Some(map);
        }
        idx
    }

    pub fn add_float(&mut self, x: f64) -> u32 {
        let idx = self.floats.len() as u32;
        self.floats.push(x);
        idx
    }

    pub fn add_struct(&mut self, desc: StructDesc) -> u32 {
        let idx = self.structs.len() as u32;
        self.structs.push(desc);
        idx
    }

    pub fn add_pattern(&mut self, pat: Pattern) -> u32 {
        let idx = self.patterns.len() as u32;
        self.patterns.push(pat);
        idx
    }

    pub fn add_proto(&mut self, proto: Rc<FnProto>) -> u32 {
        let idx = self.protos.len() as u32;
        self.protos.push(proto);
        idx
    }

    pub fn add_exports(&mut self, names: Vec<String>) -> u32 {
        let idx = self.export_lists.len() as u32;
        self.export_lists.push(names);
        idx
    }
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
    None,
    EmptyList,

    GetLocal(u32),
    SetLocal(u32),
    GetCapture(u32),
    GetGlobal(u32),
    DefGlobal(u32),
    DefFn(u32, u32), // (名前シンボルID, プロトタイプ添字)

    MakeList(u32),
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
    Jump(usize),
    JumpIfFalse(usize),
    JumpIfFalsePeek(usize),
    JumpIfTruePeek(usize),
    Pop,

    TryMatch {
        pat_idx: u32,
        fail_ip: usize,
    },
    MatchFail,

    Import(u32),
    Export(u32),
}
