//! バイトコード VM の命令列、定数チャンク、関数プロトタイプ、シンボルテーブルを定義する。

use std::cell::OnceCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::Pattern;
use crate::error::Span;

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

#[derive(Debug, Clone, Default)]
pub(crate) struct SymTable {
    pub names: Vec<Rc<str>>,
    ids: HashMap<String, u32>,
}

impl SymTable {
    pub fn intern(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = self.names.len() as u32;
        self.names.push(Rc::from(name));
        self.ids.insert(name.to_string(), id);
        id
    }

    pub fn get(&self, name: &str) -> Option<u32> {
        self.ids.get(name).copied()
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
    pub name: Box<str>,
    pub parent: Option<usize>,
}

/// コンパイル終了時に一度だけ確定する共有配列と、GetGlobal ごとの先頭。
#[derive(Debug, Clone)]
pub(crate) struct NameInfo {
    pub nodes: Rc<OnceCell<Vec<NameNode>>>,
    // 参照位置が 1 個なら追加の Vec バッファを確保しない。
    pub first_site: (usize, usize),
    pub other_sites: Vec<(usize, usize)>,
}

struct VisibleNames<'a> {
    nodes: &'a [NameNode],
    head: Option<usize>,
}

impl<'a> Iterator for VisibleNames<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let node = &self.nodes[self.head?];
        self.head = node.parent;
        Some(&node.name)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Chunk {
    pub local_count: u32,
    pub code: Vec<Op>,
    pub spans: Vec<Span>,
    pub tick_spans: Vec<Span>,
    pub strings: Vec<Rc<str>>,
    string_map: Option<HashMap<Rc<str>, u32>>,
    pub floats: Vec<f64>,
    pub structs: Vec<StructDesc>,
    pub patterns: Vec<Pattern>,
    pub protos: Vec<Rc<FnProto>>,
    pub export_lists: Vec<Vec<String>>,
    pub names: Option<Box<NameInfo>>,
}

impl Chunk {
    pub fn visible_names(&self, ip: usize) -> impl Iterator<Item = &str> {
        let (nodes, head) = self.names.as_deref().map_or((&[][..], None), |names| {
            let head = if ip == names.first_site.0 {
                Some(names.first_site.1)
            } else {
                names
                    .other_sites
                    .binary_search_by_key(&ip, |(site, _)| *site)
                    .ok()
                    .map(|index| names.other_sites[index].1)
            };
            (
                names.nodes.get().expect("name nodes finalized").as_slice(),
                head,
            )
        });
        VisibleNames { nodes, head }
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
            let mut map = HashMap::with_capacity(self.strings.len() * 2);
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
