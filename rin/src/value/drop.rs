//! 最後の所有者だけが子を取り出し、明示的スタックで解放する。
use super::*;

// Weak が残っていても、最後の強参照なら切り離して子を取り出す。
fn unique<T: Clone>(value: &mut Rc<T>) -> Option<&mut T> {
    if Rc::strong_count(value) == 1 {
        Some(Rc::make_mut(value))
    } else {
        None
    }
}

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

fn list_children(node: &mut ListNode, pending: &mut crate::work::Work<Value>) {
    if let ListNode::Cons { head, tail, .. } = node {
        if branch(head) {
            pending.push(std::mem::replace(head, Value::Void));
        }
        // 空リストは再投入しない。通常の Drop が再入しても仕事は残っていない。
        if !matches!(**tail, ListNode::Nil) {
            pending.push(Value::List(std::mem::replace(tail, ListNode::empty())));
        }
    }
}

fn children(value: &mut Value, pending: &mut crate::work::Work<Value>) {
    match value {
        Value::List(xs) => {
            if let Some(node) = unique(xs) {
                list_children(node, pending);
            }
        }
        Value::Array(xs) | Value::Tuple(xs) => {
            if let Some(xs) = unique(xs) {
                pending.extend(xs.drain(..));
            }
        }
        Value::Struct { fields, .. } => {
            if let Some(fields) = unique(fields) {
                pending.extend(std::mem::take(fields).into_values());
            }
        }
        Value::Some(v) | Value::Ok(v) | Value::Err(v) | Value::Variant { data: Some(v), .. } => {
            if let Some(v) = unique(v)
                && !matches!(v, Value::Void)
            {
                pending.push(std::mem::replace(v, Value::Void));
            }
        }
        _ => {}
    }
}

fn drain(pending: &mut crate::work::Work<Value>) {
    while let Some(mut value) = pending.pop() {
        children(&mut value, pending);
        // 子を取り出した後なので、自動 Drop が深い値へ再進入することはない。
    }
}

fn has_children(value: &Value) -> bool {
    match value {
        Value::List(v) => Rc::strong_count(v) == 1 && !v.is_empty(),
        Value::Array(v) | Value::Tuple(v) => Rc::strong_count(v) == 1 && !v.is_empty(),
        Value::Struct { fields, .. } => Rc::strong_count(fields) == 1 && !fields.is_empty(),
        Value::Some(v) | Value::Ok(v) | Value::Err(v) | Value::Variant { data: Some(v), .. } => {
            Rc::strong_count(v) == 1 && !matches!(**v, Value::Void)
        }
        _ => false,
    }
}

impl Drop for Value {
    fn drop(&mut self) {
        if !has_children(self) {
            return;
        }
        let mut pending = crate::work::Work::new();
        children(self, &mut pending);
        drain(&mut pending);
    }
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
        list_children(self, &mut pending);
        drain(&mut pending);
    }
}
