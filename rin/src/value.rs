//! ホスト境界で受け渡しする不変値 `Value` とコンスリスト `ConsList` を定義する。

use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

mod drop;
mod format;
mod owner;
pub use owner::{ValueChild, ValueFields, ValueItems};

/// `cons`・`car`・`cdr`・`len` を O(1) で行う永続コンスリスト。
#[derive(Clone)]
pub enum ListNode {
    Nil,
    Cons {
        head: Value,
        tail: Rc<ListNode>,
        len: usize,
    },
}

impl ListNode {
    pub fn empty() -> Rc<Self> {
        Rc::new(Self::Nil)
    }

    pub fn cons(head: Value, tail: Rc<Self>) -> Rc<Self> {
        let len = tail.len() + 1;
        Rc::new(Self::Cons { head, tail, len })
    }

    pub fn from_vec(items: Vec<Value>) -> Rc<Self> {
        let mut acc = Self::empty();
        for item in items.into_iter().rev() {
            acc = Self::cons(item, acc);
        }
        acc
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Nil => 0,
            Self::Cons { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Nil)
    }

    pub fn head(&self) -> Option<&Value> {
        match self {
            Self::Nil => None,
            Self::Cons { head, .. } => Some(head),
        }
    }

    pub fn tail(&self) -> Rc<Self> {
        match self {
            Self::Nil => Self::empty(),
            Self::Cons { tail, .. } => Rc::clone(tail),
        }
    }

    pub fn get(&self, mut index: usize) -> Option<&Value> {
        let mut cur = self;
        while let Self::Cons { head, tail, .. } = cur {
            if index == 0 {
                return Some(head);
            }
            index -= 1;
            cur = tail;
        }
        None
    }

    pub fn iter(&self) -> ListIter<'_> {
        ListIter { cur: self }
    }

    pub fn to_vec(&self) -> Vec<Value> {
        self.iter().cloned().collect()
    }
}

impl PartialEq for ListNode {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        self.iter().eq(other.iter())
    }
}

pub struct ListIter<'a> {
    cur: &'a ListNode,
}

impl<'a> Iterator for ListIter<'a> {
    type Item = &'a Value;

    fn next(&mut self) -> Option<Self::Item> {
        match self.cur {
            ListNode::Nil => None,
            ListNode::Cons { head, tail, .. } => {
                self.cur = tail;
                Some(head)
            }
        }
    }
}

/// Rin の不変な値表現。
#[derive(Clone)]
pub enum Value {
    Void,
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Text(Rc<String>),
    Ident(Rc<String>),
    List(Rc<ListNode>),
    Array(Rc<ValueItems>),
    Tuple(Rc<ValueItems>),
    Struct {
        name: Rc<String>,
        fields: Rc<ValueFields>,
    },
    Variant {
        type_name: Rc<String>,
        variant: Rc<String>,
        data: Option<Rc<ValueChild>>,
    },
    None,
    Some(Rc<ValueChild>),
    Ok(Rc<ValueChild>),
    Err(Rc<ValueChild>),
    Builtin(&'static str),
}

impl Value {
    pub fn some(value: Value) -> Self {
        Self::Some(Rc::new(value.into()))
    }
    pub fn ok(value: Value) -> Self {
        Self::Ok(Rc::new(value.into()))
    }
    pub fn err(value: Value) -> Self {
        Self::Err(Rc::new(value.into()))
    }

    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(Rc::new(s.into()))
    }

    pub fn ident(s: impl Into<String>) -> Self {
        Self::Ident(Rc::new(s.into()))
    }

    pub fn list(items: Vec<Value>) -> Self {
        Self::List(ListNode::from_vec(items))
    }

    pub fn empty_list() -> Self {
        Self::List(ListNode::empty())
    }

    pub fn cons(head: Value, tail: Rc<ListNode>) -> Self {
        Self::List(ListNode::cons(head, tail))
    }

    pub fn array(items: Vec<Value>) -> Self {
        Self::Array(Rc::new(items.into()))
    }

    pub fn tuple(items: Vec<Value>) -> Self {
        Self::Tuple(Rc::new(items.into()))
    }

    pub fn structure(name: impl Into<String>, fields: BTreeMap<String, Value>) -> Self {
        Self::Struct {
            name: Rc::new(name.into()),
            fields: Rc::new(fields.into()),
        }
    }

    pub fn variant(
        type_name: impl Into<String>,
        variant: impl Into<String>,
        data: Option<Value>,
    ) -> Self {
        Self::Variant {
            type_name: Rc::new(type_name.into()),
            variant: Rc::new(variant.into()),
            data: data.map(|v| Rc::new(v.into())),
        }
    }

    /// メモリ予算管理用の概算バイトサイズを返す。
    pub fn approx_bytes(&self) -> usize {
        let mut work = crate::work::Work::<_, 8>::with(self);
        let mut size = 0usize;
        while let Some(v) = work.pop() {
            let own = match v {
                Value::Void | Value::None | Value::Builtin(_) => 8,
                Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Char(_) => 16,
                Value::Text(s) | Value::Ident(s) => 24 + s.len(),
                Value::List(xs) => {
                    work.extend(xs.iter());
                    24
                }
                Value::Array(xs) | Value::Tuple(xs) => {
                    work.extend(xs.iter());
                    24
                }
                Value::Struct { name, fields } => {
                    for (k, v) in fields.iter() {
                        size = size.saturating_add(24 + k.len());
                        work.push(v);
                    }
                    32 + name.len()
                }
                Value::Variant {
                    type_name,
                    variant,
                    data,
                } => {
                    if let Some(v) = data {
                        work.push(v);
                    }
                    32 + type_name.len() + variant.len()
                }
                Value::Some(v) | Value::Ok(v) | Value::Err(v) => {
                    work.push(v);
                    16
                }
            };
            size = size.saturating_add(own);
        }
        size
    }

    pub fn type_name(&self) -> &str {
        match self {
            Value::Void => "void",
            Value::Int(_) => "i64",
            Value::Float(_) => "f64",
            Value::Bool(_) => "bool",
            Value::Char(_) => "char",
            Value::Text(_) => "text",
            Value::Ident(_) => "ident",
            Value::List(_) => "list",
            Value::Array(_) => "array",
            Value::Tuple(_) => "tuple",
            Value::Struct { name, .. } => name.as_str(),
            Value::Variant { type_name, .. } => type_name.as_str(),
            Value::None | Value::Some(_) => "option",
            Value::Ok(_) | Value::Err(_) => "result",
            Value::Builtin(_) => "fn",
        }
    }
}

impl PartialEq for Value {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        equal(self, other, 0)
    }
}

#[inline(always)]
fn equal(a: &Value, b: &Value, depth: usize) -> bool {
    match (a, b) {
        (Value::Void, Value::Void) | (Value::None, Value::None) => true,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Char(a), Value::Char(b)) => a == b,
        (Value::Text(a), Value::Text(b)) => a == b,
        (Value::Ident(a), Value::Ident(b)) => a == b,
        (Value::Builtin(a), Value::Builtin(b)) => a == b,
        _ => equal_nested(a, b, depth),
    }
}

#[inline]
fn equal_nested(a: &Value, b: &Value, depth: usize) -> bool {
    if depth == 16 {
        return equal_deep(a, b);
    }
    match (a, b) {
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| equal(a, b, depth + 1))
        }
        (Value::Array(a), Value::Array(b)) | (Value::Tuple(a), Value::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| equal(a, b, depth + 1))
        }
        (
            Value::Struct {
                name: an,
                fields: a,
            },
            Value::Struct {
                name: bn,
                fields: b,
            },
        ) => {
            if an != bn || a.len() != b.len() {
                return false;
            }
            // 1 フィールドのノードでは二つの木イテレータを作らない。
            if a.len() == 1 {
                let (ak, av) = a.first_key_value().unwrap();
                let (bk, bv) = b.first_key_value().unwrap();
                return ak == bk && equal(av, bv, depth + 1);
            }
            a.iter()
                .zip(b.iter())
                .all(|((ak, av), (bk, bv))| ak == bk && equal(av, bv, depth + 1))
        }
        (Value::Some(a), Value::Some(b))
        | (Value::Ok(a), Value::Ok(b))
        | (Value::Err(a), Value::Err(b)) => equal(a, b, depth + 1),
        (
            Value::Variant {
                type_name: at,
                variant: an,
                data: a,
            },
            Value::Variant {
                type_name: bt,
                variant: bn,
                data: b,
            },
        ) => {
            at == bt
                && an == bn
                && match (a, b) {
                    (None, None) => true,
                    (Some(a), Some(b)) => equal(a, b, depth + 1),
                    _ => false,
                }
        }
        _ => false,
    }
}

#[cold]
#[inline(never)]
fn equal_deep(a: &Value, b: &Value) -> bool {
    let mut work = crate::work::Work::<_, 8>::with((a, b));
    while let Some((a, b)) = work.pop() {
        match (a, b) {
            (Value::Void, Value::Void) | (Value::None, Value::None) => {}
            (Value::Int(a), Value::Int(b)) if a == b => {}
            (Value::Float(a), Value::Float(b)) if a == b => {}
            (Value::Bool(a), Value::Bool(b)) if a == b => {}
            (Value::Char(a), Value::Char(b)) if a == b => {}
            (Value::Text(a), Value::Text(b)) if a == b => {}
            (Value::Ident(a), Value::Ident(b)) if a == b => {}
            (Value::Builtin(a), Value::Builtin(b)) if a == b => {}
            (Value::List(a), Value::List(b)) => {
                if a.len() != b.len() {
                    return false;
                }
                work.extend(a.iter().zip(b.iter()));
            }
            (Value::Array(a), Value::Array(b)) | (Value::Tuple(a), Value::Tuple(b)) => {
                if a.len() != b.len() {
                    return false;
                }
                work.extend(a.iter().zip(b.iter()));
            }
            (
                Value::Struct {
                    name: an,
                    fields: a,
                },
                Value::Struct {
                    name: bn,
                    fields: b,
                },
            ) => {
                if an != bn || a.len() != b.len() {
                    return false;
                }
                for ((ak, av), (bk, bv)) in a.iter().zip(b.iter()) {
                    if ak != bk {
                        return false;
                    }
                    work.push((av, bv));
                }
            }
            (Value::Some(a), Value::Some(b))
            | (Value::Ok(a), Value::Ok(b))
            | (Value::Err(a), Value::Err(b)) => work.push((a, b)),
            (
                Value::Variant {
                    type_name: at,
                    variant: an,
                    data: a,
                },
                Value::Variant {
                    type_name: bt,
                    variant: bn,
                    data: b,
                },
            ) => {
                if at != bt || an != bn {
                    return false;
                }
                match (a, b) {
                    (Some(a), Some(b)) => work.push((a, b)),
                    (None, None) => {}
                    _ => return false,
                }
            }
            _ => return false,
        }
    }
    true
}
