use std::rc::Rc;

use crate::ast::Pattern;
use crate::error::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    Local(u16),
    Upvalue(u16),
}

#[derive(Debug, Clone)]
pub(crate) struct StructDesc {
    pub name: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct FnProto {
    pub name: Option<String>,
    pub module: Option<String>,
    pub arity: u16,
    pub local_count: u16,
    pub captures: Vec<Capture>,
    pub chunk: Chunk,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Chunk {
    pub local_count: u16,
    pub code: Vec<Op>,
    pub spans: Vec<Span>,
    pub strings: Vec<String>,
    pub floats: Vec<f64>,
    pub structs: Vec<StructDesc>,
    pub patterns: Vec<Pattern>,
    pub protos: Vec<Rc<FnProto>>,
    pub export_lists: Vec<Vec<String>>,
}

impl Chunk {
    pub fn emit(&mut self, op: Op, span: Span) -> usize {
        let idx = self.code.len();
        self.code.push(op);
        self.spans.push(span);
        idx
    }

    pub fn add_string(&mut self, s: String) -> u32 {
        if let Some(pos) = self.strings.iter().position(|x| x == &s) {
            return pos as u32;
        }
        let idx = self.strings.len() as u32;
        self.strings.push(s);
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
    Void,
    Int(i64),
    Float(u32),
    Bool(bool),
    Char(char),
    Str(u32),
    None,
    EmptyList,

    GetLocal(u16),
    SetLocal(u16),
    GetCapture(u16),
    GetGlobal(u32),
    DefGlobal(u32),
    DefFn(u32, u32), // (name_str_idx, proto_idx)

    MakeList(u16),
    MakeTuple(u16),
    MakeStruct(u32),
    WithStruct(u32),
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

    Call(u16),
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
