//! 表示用の継続をヒープに置き、子の Display / Debug を再帰的に呼ばない。
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
                        f.write_str(", ")?;
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
                        (false, false) => "(",
                    })?;
                    work.push(Task::Text(if debug {
                        "])"
                    } else if array {
                        "]"
                    } else {
                        ")"
                    }));
                    for (i, x) in xs.iter().enumerate().rev() {
                        work.push(Task::Value(x));
                        if i > 0 {
                            work.push(Task::Text(", "));
                        }
                    }
                }
                Value::Struct { name, fields } => {
                    if debug {
                        write!(f, "Struct {{ name: {name:?}, fields: {{")?;
                    } else {
                        write!(f, "{name} {{ ")?;
                    }
                    work.push(Task::Text(if debug { "} }" } else { " }" }));
                    for (i, (k, v)) in fields.iter().enumerate().rev() {
                        work.push(Task::Value(v));
                        work.push(Task::Text(": "));
                        work.push(if debug {
                            Task::Quoted(k)
                        } else {
                            Task::Text(k)
                        });
                        if i > 0 {
                            work.push(Task::Text(", "));
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
        format(Task::Value(self), f, false)
    }
}
impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format(Task::Value(self), f, true)
    }
}
impl fmt::Debug for ListNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        format(Task::Node(self), f, true)
    }
}
