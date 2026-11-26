//! 最後の所有者だけが子を取り出し、固定深度以降は明示的スタックで解放する。
use super::*;
use std::cell::LazyCell;
type Empty = LazyCell<Rc<ListNode>>;

// Weak が残っていても、最後の強参照なら切り離して子を取り出す。
fn unique<T: Clone>(value: &mut Rc<T>) -> Option<&mut T> {
    if Rc::strong_count(value) == 1 {
        Some(Rc::make_mut(value))
    } else {
        None
    }
}

#[inline]
fn branch(value: &Value) -> bool {
    matches!(
        value,
        Value::List(_)
            | Value::Array(_)
            | Value::Tuple(_)
            | Value::Struct { .. }
            | Value::Some(_)
            | Value::Ok(_)
            | Value::Err(_)
            | Value::Variant { data: Some(_), .. }
    )
}

fn list_children(node: &mut ListNode, pending: &mut crate::work::Work<Value>, empty: &Empty) {
    if let ListNode::Cons { head, tail, .. } = node {
        if branch(head) {
            pending.push(std::mem::replace(head, Value::Void));
        }
        // 空リストは再投入しない。通常の Drop が再入しても仕事は残っていない。
        if !matches!(**tail, ListNode::Nil) {
            pending.push(Value::List(std::mem::replace(tail, Rc::clone(empty))));
        }
    }
}

fn children(value: &mut Value, pending: &mut crate::work::Work<Value>, empty: &Empty) {
    match value {
        Value::List(xs) => {
            if let Some(node) = unique(xs) {
                list_children(node, pending, empty);
            }
        }
        Value::Array(xs) | Value::Tuple(xs) => {
            if let Some(xs) = unique(xs) {
                for v in xs.drain(..) {
                    if branch(&v) {
                        pending.push(v);
                    }
                }
            }
        }
        Value::Struct { fields, .. } => {
            if let Some(fields) = unique(fields) {
                for v in std::mem::take(&mut fields.0).into_values() {
                    if branch(&v) {
                        pending.push(v);
                    }
                }
            }
        }
        Value::Some(v) | Value::Ok(v) | Value::Err(v) | Value::Variant { data: Some(v), .. } => {
            if let Some(v) = unique(v)
                && !matches!(v.0, Value::Void)
            {
                pending.push(std::mem::replace(&mut v.0, Value::Void));
            }
        }
        _ => {}
    }
}

fn drain(pending: &mut crate::work::Work<Value>, empty: &Empty) {
    while let Some(mut value) = pending.pop() {
        children(&mut value, pending, empty);
        // 子を取り出した後なので、自動 Drop が深い値へ再進入することはない。
    }
}

#[inline]
pub(super) fn items(xs: &mut Vec<Value>) {
    for v in std::mem::take(xs) {
        release(v, 1);
    }
}

#[inline]
pub(super) fn fields(fields: &mut BTreeMap<String, Value>) {
    for v in std::mem::take(fields).into_values() {
        release(v, 1);
    }
}

#[inline]
pub(super) fn child(value: &mut Value) {
    release(std::mem::replace(value, Value::Void), 1);
}

impl Drop for ListNode {
    fn drop(&mut self) {
        let Self::Cons { head, tail, .. } = self else {
            return;
        };
        if !branch(head) && (Rc::strong_count(tail) > 1 || tail.is_empty()) {
            return;
        }
        let mut pending = crate::work::Work::new();
        let empty = LazyCell::new(ListNode::empty as fn() -> Rc<ListNode>);
        list_children(self, &mut pending, &empty);
        drain(&mut pending, &empty);
    }
}

#[cold]
#[inline(never)]
fn drop_deep(value: &mut Value) {
    let mut pending = crate::work::Work::new();
    let empty = LazyCell::new(ListNode::empty as fn() -> Rc<ListNode>);
    children(value, &mut pending, &empty);
    drain(&mut pending, &empty);
}

// Value 自身に Drop を持たせず、所有する Rc を一度だけ消費する。
// 共有されていれば参照を減らすだけ。最後の所有者なら子を取り出し、深さを引き継ぐ。
#[inline(always)]
fn release(mut value: Value, depth: usize) {
    if depth == 32 {
        drop_deep(&mut value);
        return;
    }
    match value {
        Value::Array(xs) | Value::Tuple(xs) => {
            if let Some(mut xs) = Rc::into_inner(xs) {
                for v in std::mem::take(&mut xs.0) {
                    release(v, depth + 1);
                }
            }
        }
        Value::Struct { fields, .. } => {
            if let Some(mut fields) = Rc::into_inner(fields) {
                for v in std::mem::take(&mut fields.0).into_values() {
                    release(v, depth + 1);
                }
            }
        }
        Value::Some(v) | Value::Ok(v) | Value::Err(v) => {
            if let Some(mut v) = Rc::into_inner(v) {
                release(std::mem::replace(&mut v.0, Value::Void), depth + 1);
            }
        }
        Value::Variant { data: Some(v), .. } => {
            if let Some(mut v) = Rc::into_inner(v) {
                release(std::mem::replace(&mut v.0, Value::Void), depth + 1);
            }
        }
        Value::List(xs) => drop(xs), // ListNode の反復解放が部分木全体を処理する。
        Value::Str(s) | Value::Ident(s) => drop(s),
        Value::Variant { data: None, .. } => {}
        Value::Void
        | Value::None
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::Char(_)
        | Value::Builtin(_) => {}
    }
}
