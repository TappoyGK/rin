//! 浅い部分は直接表示し、固定深度以降の継続は明示的スタックに置く。
use super::*;

enum Task<'a> {
    Value(&'a Value),
    Node(&'a ListNode),
    List(&'a ListNode, bool),
    Text(&'a str),
    Quoted(&'a str),
    Number(usize),
}

fn format(root: Task<'_>, f: &mut fmt::Formatter<'_>, debug: bool) -> fmt::Result {
    let mut work = crate::work::Work::<_, 8>::with(root);
    while let Some(task) = work.pop() {
        match task {
            Task::Text(s) => f.write_str(s)?,
            Task::Quoted(s) => write!(f, "{s:?}")?,
            Task::Number(n) => write!(f, "{n}")?,
            Task::List(node, first) => match node {
                ListNode::Nil => {}
                ListNode::Cons { head, tail, .. } => {
                    if !first {
                        f.write_str(" ")?;
                    }
                    work.push(Task::List(tail, false));
                    work.push(Task::Value(head));
                }
            },
            Task::Node(node) => match node {
                ListNode::Nil => f.write_str("Nil")?,
                ListNode::Cons { head, tail, len } => {
                    f.write_str("Cons { head: ")?;
                    work.extend([
                        Task::Text(" }"),
                        Task::Number(*len),
                        Task::Text(", len: "),
                        Task::Node(tail),
                        Task::Text(", tail: "),
                        Task::Value(head),
                    ]);
                }
            },
            Task::Value(v) => match v {
                Value::Void => f.write_str(if debug { "Void" } else { "void" })?,
                Value::None => f.write_str(if debug { "None" } else { "none" })?,
                Value::Int(n) => {
                    if debug {
                        write!(f, "Int({n})")?
                    } else {
                        write!(f, "{n}")?
                    }
                }
                Value::Float(n) => {
                    if debug {
                        write!(f, "Float({n:?})")?
                    } else {
                        write!(f, "{n}")?
                    }
                }
                Value::Bool(b) => {
                    if debug {
                        write!(f, "Bool({b})")?
                    } else {
                        write!(f, "{b}")?
                    }
                }
                Value::Char(c) => {
                    if debug {
                        write!(f, "Char({c:?})")?
                    } else {
                        write!(f, "{c:?}")?
                    }
                }
                Value::Str(s) => {
                    if debug {
                        write!(f, "Str({s:?})")?
                    } else {
                        write!(f, "{s:?}")?
                    }
                }
                Value::Ident(s) => {
                    if debug {
                        write!(f, "Ident({s:?})")?
                    } else {
                        write!(f, "'{s}")?
                    }
                }
                Value::Builtin(s) => {
                    if debug {
                        write!(f, "Builtin({s:?})")?
                    } else {
                        write!(f, "<builtin {s}>")?
                    }
                }
                Value::List(xs) => {
                    f.write_str(if debug { "List(" } else { "[" })?;
                    work.push(Task::Text(if debug { ")" } else { "]" }));
                    work.push(if debug {
                        Task::Node(xs)
                    } else {
                        Task::List(xs, true)
                    });
                }
                Value::Array(xs) | Value::Tuple(xs) => {
                    let array = matches!(v, Value::Array(_));
                    f.write_str(match (debug, array) {
                        (true, true) => "Array([",
                        (true, false) => "Tuple([",
                        (false, true) => "#[",
                        (false, false) => {
                            if xs.is_empty() {
                                "(tuple"
                            } else {
                                "(tuple "
                            }
                        }
                    })?;
                    work.push(Task::Text(if debug {
                        "])"
                    } else if array {
                        "]"
                    } else {
                        ")"
                    }));
                    let sep = if debug { ", " } else { " " };
                    for (i, x) in xs.iter().enumerate().rev() {
                        work.push(Task::Value(x));
                        if i > 0 {
                            work.push(Task::Text(sep));
                        }
                    }
                }
                Value::Struct { name, fields } => {
                    if debug {
                        write!(f, "Struct {{ name: {name:?}, fields: {{")?;
                        work.push(Task::Text("} }"));
                    } else if fields.is_empty() {
                        write!(f, "{name} {{}}")?;
                        continue;
                    } else {
                        write!(f, "{name} {{ ")?;
                        work.push(Task::Text(" }"));
                    }
                    let sep = if debug { ", " } else { " " };
                    for (i, (k, v)) in fields.iter().enumerate().rev() {
                        work.push(Task::Value(v));
                        work.push(Task::Text(": "));
                        work.push(if debug {
                            Task::Quoted(k)
                        } else {
                            Task::Text(k)
                        });
                        if i > 0 {
                            work.push(Task::Text(sep));
                        }
                    }
                }
                Value::Variant {
                    type_name,
                    variant,
                    data,
                } => {
                    if debug {
                        write!(
                            f,
                            "Variant {{ type_name: {type_name:?}, variant: {variant:?}, data: "
                        )?;
                        work.push(Task::Text(" }"));
                        if let Some(v) = data {
                            f.write_str("Some(")?;
                            work.push(Task::Text(")"));
                            work.push(Task::Value(v));
                        } else {
                            f.write_str("None")?;
                        }
                    } else {
                        write!(f, "{type_name}.{variant}")?;
                        if let Some(v) = data {
                            f.write_str("(")?;
                            work.push(Task::Text(")"));
                            work.push(Task::Value(v));
                        }
                    }
                }
                Value::Some(x) | Value::Ok(x) | Value::Err(x) => {
                    f.write_str(match (v, debug) {
                        (Value::Some(_), true) => "Some(",
                        (Value::Some(_), false) => "some(",
                        (Value::Ok(_), true) => "Ok(",
                        (Value::Ok(_), false) => "ok(",
                        (_, true) => "Err(",
                        (_, false) => "err(",
                    })?;
                    work.push(Task::Text(")"));
                    work.push(Task::Value(x));
                }
            },
        }
    }
    Ok(())
}
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        display(self, f, 0)
    }
}
impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        DebugAt(self, 0).fmt(f)
    }
}
impl fmt::Debug for ListNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        DebugNodeAt(self, 0).fmt(f)
    }
}

struct DisplayAt<'a>(&'a Value, usize);
impl fmt::Display for DisplayAt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        display(self.0, f, self.1)
    }
}
fn display(value: &Value, f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
    if depth == 16 {
        return format(Task::Value(value), f, false);
    }
    match value {
        Value::Void => write!(f, "void"),
        Value::Int(n) => write!(f, "{n}"),
        Value::Float(x) => write!(f, "{x}"),
        Value::Bool(b) => write!(f, "{b}"),
        Value::Char(c) => write!(f, "{c:?}"),
        Value::Str(s) => write!(f, "{s:?}"),
        Value::Ident(s) => write!(f, "'{s}"),
        Value::List(xs) => {
            write!(f, "[")?;
            for (i, x) in xs.iter().enumerate() {
                let x = DisplayAt(x, depth + 1);
                if i > 0 {
                    write!(f, " ")?;
                }
                write!(f, "{x}")?;
            }
            write!(f, "]")
        }
        Value::Array(xs) => {
            write!(f, "#[")?;
            for (i, x) in xs.iter().enumerate() {
                let x = DisplayAt(x, depth + 1);
                if i > 0 {
                    write!(f, " ")?;
                }
                write!(f, "{x}")?;
            }
            write!(f, "]")
        }
        Value::Tuple(xs) => {
            write!(f, "(tuple")?;
            for x in xs.iter() {
                let x = DisplayAt(x, depth + 1);
                write!(f, " {x}")?;
            }
            write!(f, ")")
        }
        Value::Struct { name, fields } => {
            if fields.is_empty() {
                return write!(f, "{name} {{}}");
            }
            write!(f, "{name} {{ ")?;
            for (i, (k, v)) in fields.iter().enumerate() {
                let v = DisplayAt(v, depth + 1);
                if i > 0 {
                    write!(f, " ")?;
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
                let d = DisplayAt(d, depth + 1);
                write!(f, "({d})")?;
            }
            Ok(())
        }
        Value::None => write!(f, "none"),
        Value::Some(v) => write!(f, "some({})", DisplayAt(v, depth + 1)),
        Value::Ok(v) => write!(f, "ok({})", DisplayAt(v, depth + 1)),
        Value::Err(v) => write!(f, "err({})", DisplayAt(v, depth + 1)),
        Value::Builtin(name) => write!(f, "<builtin {name}>"),
    }
}

struct DebugAt<'a>(&'a Value, usize);
struct DebugNodeAt<'a>(&'a ListNode, usize);
struct DebugItems<'a>(&'a [Value], usize);
struct DebugFields<'a>(&'a BTreeMap<String, Value>, usize);
impl fmt::Debug for DebugItems<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|v| DebugAt(v, self.1)))
            .finish()
    }
}
impl fmt::Debug for DebugFields<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.0.iter().map(|(k, v)| (k, DebugAt(v, self.1))))
            .finish()
    }
}
impl fmt::Debug for DebugNodeAt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.1 == 16 {
            return format(Task::Node(self.0), f, true);
        }
        match self.0 {
            ListNode::Nil => f.write_str("Nil"),
            ListNode::Cons { head, tail, len } => f
                .debug_struct("Cons")
                .field("head", &DebugAt(head, self.1 + 1))
                .field("tail", &DebugNodeAt(tail, self.1 + 1))
                .field("len", len)
                .finish(),
        }
    }
}
impl fmt::Debug for DebugAt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.1 == 16 {
            return format(Task::Value(self.0), f, true);
        }
        let d = self.1 + 1;
        match self.0 {
            Value::Void => f.write_str("Void"),
            Value::None => f.write_str("None"),
            Value::Int(v) => f.debug_tuple("Int").field(v).finish(),
            Value::Float(v) => f.debug_tuple("Float").field(v).finish(),
            Value::Bool(v) => f.debug_tuple("Bool").field(v).finish(),
            Value::Char(v) => f.debug_tuple("Char").field(v).finish(),
            Value::Str(v) => f.debug_tuple("Str").field(v).finish(),
            Value::Ident(v) => f.debug_tuple("Ident").field(v).finish(),
            Value::Builtin(v) => f.debug_tuple("Builtin").field(v).finish(),
            Value::Some(v) => f.debug_tuple("Some").field(&DebugAt(v, d)).finish(),
            Value::Ok(v) => f.debug_tuple("Ok").field(&DebugAt(v, d)).finish(),
            Value::Err(v) => f.debug_tuple("Err").field(&DebugAt(v, d)).finish(),
            Value::List(v) => f.debug_tuple("List").field(&DebugNodeAt(v, d)).finish(),
            Value::Array(v) => f.debug_tuple("Array").field(&DebugItems(v, d)).finish(),
            Value::Tuple(v) => f.debug_tuple("Tuple").field(&DebugItems(v, d)).finish(),
            Value::Struct { name, fields } => f
                .debug_struct("Struct")
                .field("name", name)
                .field("fields", &DebugFields(fields, d))
                .finish(),
            Value::Variant {
                type_name,
                variant,
                data,
            } => f
                .debug_struct("Variant")
                .field("type_name", type_name)
                .field("variant", variant)
                .field("data", &data.as_ref().map(|v| DebugAt(v, d)))
                .finish(),
        }
    }
}
