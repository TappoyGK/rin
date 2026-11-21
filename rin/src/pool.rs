use std::collections::BTreeMap;
use std::rc::Rc;

use crate::alloc::{AllocBuf, Allocator};
use crate::bytecode::{BuiltinId, FnProto};
use crate::error::{Error, ErrorKind, Span};
use crate::value::Value;

const CHUNK_SLOTS: usize = 256;

/// Handle to a refcounted object inside Rin's internal pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObjId(pub u32);

/// Compact runtime value stored in variables, stacks, and pool objects.
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
                Obj::Str(_) => "string",
                Obj::Tuple(_) => "tuple",
                Obj::Struct { .. } => "struct",
                Obj::Variant { .. } => "variant",
                Obj::Some(_) => "option",
                Obj::Ok(_) | Obj::Err(_) => "result",
                Obj::Closure { .. } => "fn",
                Obj::Module { .. } => "module",
            },
        }
    }
}

pub(crate) enum Obj {
    Cons {
        head: RVal,
        tail: RVal,
        len: usize,
    },
    Array(Vec<RVal>),
    Str(Rc<str>),
    Tuple(Vec<RVal>),
    Struct {
        name: Rc<str>,
        fields: BTreeMap<Rc<str>, RVal>,
    },
    Variant {
        type_name: Rc<str>,
        variant: Rc<str>,
        data: Option<RVal>,
    },
    Some(RVal),
    Ok(RVal),
    Err(RVal),
    Closure {
        proto: Rc<FnProto>,
        captures: Rc<[RVal]>,
    },
    Module {
        name: Rc<str>,
        exports: BTreeMap<Rc<str>, RVal>,
    },
}

impl Obj {
    fn approx_bytes(&self) -> usize {
        match self {
            Obj::Cons { .. } => 40,
            Obj::Array(xs) => 32 + xs.len() * 16,
            Obj::Str(s) => 32 + s.len(),
            Obj::Tuple(xs) => 32 + xs.len() * 16,
            Obj::Struct { name, fields } => 48 + name.len() + fields.len() * 32,
            Obj::Variant {
                type_name, variant, ..
            } => 48 + type_name.len() + variant.len(),
            Obj::Some(_) | Obj::Ok(_) | Obj::Err(_) => 24,
            Obj::Closure { captures, .. } => 64 + captures.len() * 16,
            Obj::Module { name, exports } => 48 + name.len() + exports.len() * 32,
        }
    }

    fn push_children(&self, out: &mut Vec<ObjId>) {
        match self {
            Obj::Cons { head, tail, .. } => {
                if let RVal::Obj(id) = head {
                    out.push(*id);
                }
                if let RVal::Obj(id) = tail {
                    out.push(*id);
                }
            }
            Obj::Str(_) => {}
            Obj::Array(xs) | Obj::Tuple(xs) => {
                for x in xs {
                    if let RVal::Obj(id) = x {
                        out.push(*id);
                    }
                }
            }
            Obj::Struct { fields, .. } => {
                for v in fields.values() {
                    if let RVal::Obj(id) = v {
                        out.push(*id);
                    }
                }
            }
            Obj::Variant { data, .. } => {
                if let Some(RVal::Obj(id)) = data {
                    out.push(*id);
                }
            }
            Obj::Some(v) | Obj::Ok(v) | Obj::Err(v) => {
                if let RVal::Obj(id) = v {
                    out.push(*id);
                }
            }
            Obj::Closure { captures, .. } => {
                for v in captures.iter() {
                    if let RVal::Obj(id) = v {
                        out.push(*id);
                    }
                }
            }
            Obj::Module { exports, .. } => {
                for v in exports.values() {
                    if let RVal::Obj(id) = v {
                        out.push(*id);
                    }
                }
            }
        }
    }
}

enum Slot {
    Free { next: Option<u32> },
    Live { rc: u32, bytes: usize, obj: Obj },
}

/// Refcounted object pool backed by chunked slabs from a caller-provided `Allocator`.
///
/// Dead objects are returned to a free-list and reused immediately on subsequent
/// allocations, preventing monotonic memory growth within a single arena/session.
pub(crate) struct Pool<'a> {
    alloc: &'a mut dyn Allocator,
    chunks: Vec<AllocBuf<Slot>>,
    /// Number of initialized slots in the last chunk.
    last_chunk_len: usize,
    free_head: Option<u32>,
    live_bytes: usize,
    max_bytes: usize,
    release_worklist: Vec<ObjId>,
}

impl<'a> Pool<'a> {
    pub fn new(alloc: &'a mut dyn Allocator, max_bytes: usize) -> Self {
        Self {
            alloc,
            chunks: Vec::new(),
            last_chunk_len: 0,
            free_head: None,
            live_bytes: 0,
            max_bytes,
            release_worklist: Vec::new(),
        }
    }

    pub fn live_bytes(&self) -> usize {
        self.live_bytes
    }

    fn slot_ref(&self, id: ObjId) -> &Slot {
        let idx = id.0 as usize;
        let chunk_idx = idx / CHUNK_SLOTS;
        let in_chunk = idx % CHUNK_SLOTS;
        let len = if chunk_idx + 1 == self.chunks.len() {
            self.last_chunk_len
        } else {
            CHUNK_SLOTS
        };
        &self.chunks[chunk_idx].as_slice(len)[in_chunk]
    }

    fn slot_mut(&mut self, id: ObjId) -> &mut Slot {
        let idx = id.0 as usize;
        let chunk_idx = idx / CHUNK_SLOTS;
        let in_chunk = idx % CHUNK_SLOTS;
        let len = if chunk_idx + 1 == self.chunks.len() {
            self.last_chunk_len
        } else {
            CHUNK_SLOTS
        };
        &mut self.chunks[chunk_idx].as_mut_slice(len)[in_chunk]
    }

    pub fn get(&self, id: ObjId) -> &Obj {
        match self.slot_ref(id) {
            Slot::Live { obj, .. } => obj,
            Slot::Free { .. } => panic!("dangling ObjId({})", id.0),
        }
    }

    pub fn retain_val(&mut self, val: RVal) {
        if let RVal::Obj(id) = val {
            self.retain(id);
        }
    }

    pub fn retain(&mut self, id: ObjId) {
        if let Slot::Live { rc, .. } = self.slot_mut(id) {
            *rc += 1;
        }
    }

    pub fn release_val(&mut self, val: RVal) {
        if let RVal::Obj(id) = val {
            self.release(id);
        }
    }

    pub fn release(&mut self, root: ObjId) {
        let should_free_root = match self.slot_mut(root) {
            Slot::Live { rc, .. } => {
                *rc -= 1;
                *rc == 0
            }
            Slot::Free { .. } => false,
        };
        if !should_free_root {
            return;
        }
        let mut worklist = std::mem::take(&mut self.release_worklist);
        worklist.push(root);
        let mut first = true;
        while let Some(id) = worklist.pop() {
            let should_free = if first {
                first = false;
                true
            } else {
                match self.slot_mut(id) {
                    Slot::Live { rc, .. } => {
                        *rc -= 1;
                        *rc == 0
                    }
                    Slot::Free { .. } => false,
                }
            };
            if should_free {
                let next_free = self.free_head;
                let prev = std::mem::replace(self.slot_mut(id), Slot::Free { next: next_free });
                self.free_head = Some(id.0);
                if let Slot::Live { bytes, obj, .. } = prev {
                    self.live_bytes = self.live_bytes.saturating_sub(bytes);
                    obj.push_children(&mut worklist);
                }
            }
        }
        self.release_worklist = worklist;
    }

    fn release_unallocated(&mut self, obj: Obj) {
        let mut worklist = std::mem::take(&mut self.release_worklist);
        obj.push_children(&mut worklist);
        self.release_worklist = worklist;
        while let Some(id) = self.release_worklist.pop() {
            self.release(id);
        }
    }

    pub fn alloc_obj(&mut self, obj: Obj, span: Span) -> Result<ObjId, Error> {
        let bytes = obj.approx_bytes();
        let next_live = self.live_bytes.saturating_add(bytes);
        if next_live > self.max_bytes {
            self.release_unallocated(obj);
            return Err(Error::new(
                ErrorKind::TooMuchMemory,
                span,
                format!("memory limit exceeded ({} bytes)", self.max_bytes),
            ));
        }

        if let Some(free_idx) = self.free_head {
            let id = ObjId(free_idx);
            let next = match self.slot_ref(id) {
                Slot::Free { next } => *next,
                Slot::Live { .. } => unreachable!(),
            };
            self.free_head = next;
            *self.slot_mut(id) = Slot::Live { rc: 1, bytes, obj };
            self.live_bytes = next_live;
            return Ok(id);
        }

        if self.chunks.is_empty() || self.last_chunk_len == CHUNK_SLOTS {
            let Some(buf) = AllocBuf::<Slot>::with_capacity(self.alloc, CHUNK_SLOTS) else {
                self.release_unallocated(obj);
                return Err(Error::new(
                    ErrorKind::TooMuchMemory,
                    span,
                    "allocator exhausted while growing pool",
                ));
            };
            self.chunks.push(buf);
            self.last_chunk_len = 0;
        }

        let chunk_idx = self.chunks.len() - 1;
        let in_chunk = self.last_chunk_len;
        let global_idx = (chunk_idx * CHUNK_SLOTS + in_chunk) as u32;
        let slot = Slot::Live { rc: 1, bytes, obj };
        // SAFETY: `in_chunk < CHUNK_SLOTS` and is uninitialized.
        unsafe { self.chunks[chunk_idx].write_at(in_chunk, slot) };
        self.last_chunk_len += 1;
        self.live_bytes = next_live;
        Ok(ObjId(global_idx))
    }

    pub fn alloc_cons(&mut self, head: RVal, tail: RVal, span: Span) -> Result<RVal, Error> {
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

    /// Exports a runtime `RVal` into a standalone `Value`.
    pub fn export(&self, val: RVal) -> Value {
        match val {
            RVal::Void => Value::Void,
            RVal::Int(n) => Value::Int(n),
            RVal::Float(x) => Value::Float(x),
            RVal::Bool(b) => Value::Bool(b),
            RVal::Char(c) => Value::Char(c),
            RVal::None => Value::None,
            RVal::EmptyList => Value::empty_list(),
            RVal::Builtin(b) => Value::Builtin(b.name()),
            RVal::Obj(id) => match self.get(id) {
                Obj::Cons { .. } => {
                    let mut items = Vec::with_capacity(self.list_len(val));
                    let mut cur = val;
                    while let RVal::Obj(cid) = cur {
                        let Obj::Cons { head, tail, .. } = self.get(cid) else {
                            break;
                        };
                        items.push(self.export(*head));
                        cur = *tail;
                    }
                    Value::list(items)
                }
                Obj::Str(s) => Value::str(s.as_ref()),
                Obj::Array(xs) => Value::array(xs.iter().map(|v| self.export(*v)).collect()),
                Obj::Tuple(xs) => Value::tuple(xs.iter().map(|v| self.export(*v)).collect()),
                Obj::Struct { name, fields } => {
                    let map = fields
                        .iter()
                        .map(|(k, v)| (k.to_string(), self.export(*v)))
                        .collect();
                    Value::structure(name.as_ref(), map)
                }
                Obj::Variant {
                    type_name,
                    variant,
                    data,
                } => Value::variant(
                    type_name.as_ref(),
                    variant.as_ref(),
                    data.map(|d| self.export(d)),
                ),
                Obj::Some(v) => Value::Some(std::rc::Rc::new(self.export(*v))),
                Obj::Ok(v) => Value::Ok(std::rc::Rc::new(self.export(*v))),
                Obj::Err(v) => Value::Err(std::rc::Rc::new(self.export(*v))),
                Obj::Closure { proto, .. } => Value::Builtin(match proto.name.as_deref() {
                    Some(_) => "<defn>",
                    None => "<fn>",
                }),
                Obj::Module { .. } => Value::Builtin("<module>"),
            },
        }
    }

    /// Imports an external `Value` into the pool.
    pub fn import(&mut self, val: &Value, span: Span) -> Result<RVal, Error> {
        match val {
            Value::Void => Ok(RVal::Void),
            Value::Int(n) => Ok(RVal::Int(*n)),
            Value::Float(x) => Ok(RVal::Float(*x)),
            Value::Bool(b) => Ok(RVal::Bool(*b)),
            Value::Char(c) => Ok(RVal::Char(*c)),
            Value::None => Ok(RVal::None),
            Value::Builtin(name) => {
                let id = BuiltinId::ALL
                    .iter()
                    .copied()
                    .find(|b| b.name() == *name)
                    .ok_or_else(|| Error::runtime(span, format!("cannot import '{name}'")))?;
                Ok(RVal::Builtin(id))
            }
            Value::Str(s) => Ok(RVal::Obj(
                self.alloc_obj(Obj::Str(Rc::from(s.as_str())), span)?,
            )),
            Value::List(xs) => {
                let items: Vec<Value> = xs.to_vec();
                let mut acc = RVal::EmptyList;
                for item in items.iter().rev() {
                    let h = self.import(item, span)?;
                    let next = self.alloc_cons(h, acc, span);
                    self.release_val(h);
                    self.release_val(acc);
                    acc = next?;
                }
                Ok(acc)
            }
            Value::Array(xs) => {
                let mut items = Vec::with_capacity(xs.len());
                for x in xs.iter() {
                    items.push(self.import(x, span)?);
                }
                Ok(RVal::Obj(self.alloc_obj(Obj::Array(items), span)?))
            }
            Value::Tuple(xs) => {
                let mut items = Vec::with_capacity(xs.len());
                for x in xs.iter() {
                    items.push(self.import(x, span)?);
                }
                Ok(RVal::Obj(self.alloc_obj(Obj::Tuple(items), span)?))
            }
            Value::Struct { name, fields } => {
                let mut map = BTreeMap::new();
                for (k, v) in fields.iter() {
                    map.insert(Rc::from(k.as_str()), self.import(v, span)?);
                }
                Ok(RVal::Obj(self.alloc_obj(
                    Obj::Struct {
                        name: Rc::from(name.as_str()),
                        fields: map,
                    },
                    span,
                )?))
            }
            Value::Variant {
                type_name,
                variant,
                data,
            } => {
                let d = match data {
                    Some(inner) => Some(self.import(inner, span)?),
                    None => None,
                };
                Ok(RVal::Obj(self.alloc_obj(
                    Obj::Variant {
                        type_name: Rc::from(type_name.as_str()),
                        variant: Rc::from(variant.as_str()),
                        data: d,
                    },
                    span,
                )?))
            }
            Value::Some(v) => {
                let inner = self.import(v, span)?;
                Ok(RVal::Obj(self.alloc_obj(Obj::Some(inner), span)?))
            }
            Value::Ok(v) => {
                let inner = self.import(v, span)?;
                Ok(RVal::Obj(self.alloc_obj(Obj::Ok(inner), span)?))
            }
            Value::Err(v) => {
                let inner = self.import(v, span)?;
                Ok(RVal::Obj(self.alloc_obj(Obj::Err(inner), span)?))
            }
        }
    }
}

impl Drop for Pool<'_> {
    fn drop(&mut self) {
        let last_idx = self.chunks.len().saturating_sub(1);
        for (idx, mut chunk) in self.chunks.drain(..).enumerate() {
            let len = if idx == last_idx {
                self.last_chunk_len
            } else {
                CHUNK_SLOTS
            };
            chunk.destroy(self.alloc, len);
        }
    }
}
