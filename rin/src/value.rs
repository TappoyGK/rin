use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

/// Persistent cons-cell linked list with O(1) cons, car, cdr, and len.
#[derive(Debug, Clone)]
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

impl Drop for ListNode {
    fn drop(&mut self) {
        let Self::Cons { tail, .. } = self else {
            return;
        };
        let mut cur = std::mem::replace(tail, Rc::new(Self::Nil));
        while let Some(ListNode::Cons { tail, .. }) = Rc::get_mut(&mut cur) {
            cur = std::mem::replace(tail, Rc::new(Self::Nil));
        }
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

/// A Rin runtime value. All values are immutable.
#[derive(Debug, Clone)]
pub enum Value {
    Void,
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Str(Rc<String>),
    List(Rc<ListNode>),
    Array(Rc<Vec<Value>>),
    Tuple(Rc<Vec<Value>>),
    Struct {
        name: Rc<String>,
        fields: Rc<BTreeMap<String, Value>>,
    },
    Variant {
        type_name: Rc<String>,
        variant: Rc<String>,
        data: Option<Rc<Value>>,
    },
    None,
    Some(Rc<Value>),
    Ok(Rc<Value>),
    Err(Rc<Value>),
    Builtin(&'static str),
}

impl Value {
    pub fn str(s: impl Into<String>) -> Self {
        Self::Str(Rc::new(s.into()))
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
        Self::Array(Rc::new(items))
    }

    pub fn tuple(items: Vec<Value>) -> Self {
        Self::Tuple(Rc::new(items))
    }

    pub fn structure(name: impl Into<String>, fields: BTreeMap<String, Value>) -> Self {
        Self::Struct {
            name: Rc::new(name.into()),
            fields: Rc::new(fields),
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
            data: data.map(Rc::new),
        }
    }

    /// Rough byte-size estimate for memory budget accounting.
    pub fn approx_bytes(&self) -> usize {
        match self {
            Value::Void | Value::None | Value::Builtin(_) => 8,
            Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Char(_) => 16,
            Value::Str(s) => 24 + s.len(),
            Value::List(xs) => 24 + xs.iter().map(Value::approx_bytes).sum::<usize>(),
            Value::Array(xs) => 24 + xs.iter().map(Value::approx_bytes).sum::<usize>(),
            Value::Tuple(xs) => 24 + xs.iter().map(Value::approx_bytes).sum::<usize>(),
            Value::Struct { name, fields } => {
                32 + name.len()
                    + fields
                        .iter()
                        .map(|(k, v)| 24 + k.len() + v.approx_bytes())
                        .sum::<usize>()
            }
            Value::Variant {
                type_name,
                variant,
                data,
            } => {
                32 + type_name.len()
                    + variant.len()
                    + data.as_ref().map_or(0, |d| d.approx_bytes())
            }
            Value::Some(v) | Value::Ok(v) | Value::Err(v) => 16 + v.approx_bytes(),
        }
    }

    pub fn type_name(&self) -> &str {
        match self {
            Value::Void => "void",
            Value::Int(_) => "i64",
            Value::Float(_) => "f64",
            Value::Bool(_) => "bool",
            Value::Char(_) => "char",
            Value::Str(_) => "string",
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
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Void, Value::Void) => true,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => a == b,
            (Value::Tuple(a), Value::Tuple(b)) => a == b,
            (
                Value::Struct {
                    name: n1,
                    fields: f1,
                },
                Value::Struct {
                    name: n2,
                    fields: f2,
                },
            ) => n1 == n2 && f1 == f2,
            (
                Value::Variant {
                    type_name: t1,
                    variant: v1,
                    data: d1,
                },
                Value::Variant {
                    type_name: t2,
                    variant: v2,
                    data: d2,
                },
            ) => t1 == t2 && v1 == v2 && d1 == d2,
            (Value::None, Value::None) => true,
            (Value::Some(a), Value::Some(b)) => a == b,
            (Value::Ok(a), Value::Ok(b)) => a == b,
            (Value::Err(a), Value::Err(b)) => a == b,
            (Value::Builtin(a), Value::Builtin(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Void => write!(f, "void"),
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Char(c) => write!(f, "{c:?}"),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::List(xs) => {
                write!(f, "[")?;
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{x}")?;
                }
                write!(f, "]")
            }
            Value::Array(xs) => {
                write!(f, "#[")?;
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{x}")?;
                }
                write!(f, "]")
            }
            Value::Tuple(xs) => {
                write!(f, "(")?;
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{x}")?;
                }
                write!(f, ")")
            }
            Value::Struct { name, fields } => {
                write!(f, "{name} {{ ")?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, " }}")
            }
            Value::Variant {
                type_name,
                variant,
                data,
            } => {
                write!(f, "{type_name}.{variant}")?;
                if let Some(d) = data {
                    write!(f, "({d})")?;
                }
                Ok(())
            }
            Value::None => write!(f, "none"),
            Value::Some(v) => write!(f, "some({v})"),
            Value::Ok(v) => write!(f, "ok({v})"),
            Value::Err(v) => write!(f, "err({v})"),
            Value::Builtin(name) => write!(f, "<builtin {name}>"),
        }
    }
}
