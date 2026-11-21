//! ホスト境界の値変換。子の処理を作業スタックに積み、失敗時には所有する値を回収する。
use super::*;
use crate::value::ListNode;

enum ImportTask<'a> {
    Visit(&'a Value),
    List(&'a ListNode),
    Cons,
    Finish(&'a Value, usize),
}

impl Pool<'_> {
    pub fn import(&mut self, value: &Value, span: Span) -> Result<RVal, Error> {
        match value {
            Value::Void => return Ok(RVal::Void),
            Value::Int(n) => return Ok(RVal::Int(*n)),
            Value::Float(n) => return Ok(RVal::Float(*n)),
            Value::Bool(b) => return Ok(RVal::Bool(*b)),
            Value::Char(c) => return Ok(RVal::Char(*c)),
            Value::None => return Ok(RVal::None),
            _ => {}
        }
        self.import_shallow(value, span, 0)
    }

    #[inline(never)]
    pub(super) fn import_deep(&mut self, value: &Value, span: Span) -> Result<RVal, Error> {
        let mut tasks = crate::work::Work::<_, 16>::with(ImportTask::Visit(value));
        let mut values = Vec::new();
        let result = (|| {
            while let Some(task) = tasks.pop() {
                match task {
                    ImportTask::Visit(v) => {
                        let scalar = match v {
                            Value::Void => Some(RVal::Void),
                            Value::Int(n) => Some(RVal::Int(*n)),
                            Value::Float(n) => Some(RVal::Float(*n)),
                            Value::Bool(b) => Some(RVal::Bool(*b)),
                            Value::Char(c) => Some(RVal::Char(*c)),
                            Value::None => Some(RVal::None),
                            Value::Builtin(name) => Some(RVal::Builtin(
                                BuiltinId::ALL
                                    .iter()
                                    .copied()
                                    .find(|b| b.name() == *name)
                                    .ok_or_else(|| {
                                        Error::runtime(span, format!("cannot import '{name}'"))
                                    })?,
                            )),
                            Value::Str(s) => Some(RVal::Obj(
                                self.alloc_obj(Obj::Str(Rc::from(s.as_str())), span)?,
                            )),
                            _ => None,
                        };
                        if let Some(v) = scalar {
                            values.push(v);
                            continue;
                        }
                        let base = values.len();
                        match v {
                            Value::List(xs) => tasks.push(ImportTask::List(xs)),
                            Value::Array(xs) | Value::Tuple(xs) => {
                                tasks.push(ImportTask::Finish(v, base));
                                tasks.extend(xs.iter().rev().map(ImportTask::Visit));
                            }
                            Value::Struct { fields, .. } => {
                                tasks.push(ImportTask::Finish(v, base));
                                tasks.extend(fields.values().rev().map(ImportTask::Visit));
                            }
                            Value::Some(x) | Value::Ok(x) | Value::Err(x) => {
                                tasks.push(ImportTask::Finish(v, base));
                                tasks.push(ImportTask::Visit(x));
                            }
                            Value::Variant { data, .. } => {
                                tasks.push(ImportTask::Finish(v, base));
                                if let Some(x) = data {
                                    tasks.push(ImportTask::Visit(x));
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                    ImportTask::List(xs) => match xs {
                        ListNode::Nil => values.push(RVal::EmptyList),
                        ListNode::Cons { head, tail, .. } => {
                            tasks.push(ImportTask::Cons);
                            tasks.push(ImportTask::List(tail));
                            tasks.push(ImportTask::Visit(head));
                        }
                    },
                    ImportTask::Cons => {
                        let tail = values.pop().unwrap();
                        let head = values.pop().unwrap();
                        let len = self.list_len(tail) + 1;
                        // alloc_obj は成功・失敗のどちらでも子の所有を引き受ける。
                        values.push(RVal::Obj(
                            self.alloc_obj(Obj::Cons { head, tail, len }, span)?,
                        ));
                    }
                    ImportTask::Finish(v, base) => {
                        let obj = match v {
                            Value::Array(_) => Obj::Array(values.drain(base..).collect()),
                            Value::Tuple(_) => Obj::Tuple(values.drain(base..).collect()),
                            Value::Struct { name, fields } => Obj::Struct {
                                name: Rc::from(name.as_str()),
                                fields: fields
                                    .keys()
                                    .map(|k| Rc::from(k.as_str()))
                                    .zip(values.drain(base..))
                                    .collect(),
                            },
                            Value::Some(_) => Obj::Some(values.pop().unwrap()),
                            Value::Ok(_) => Obj::Ok(values.pop().unwrap()),
                            Value::Err(_) => Obj::Err(values.pop().unwrap()),
                            Value::Variant {
                                type_name,
                                variant,
                                data,
                            } => Obj::Variant {
                                type_name: Rc::from(type_name.as_str()),
                                variant: Rc::from(variant.as_str()),
                                data: data.as_ref().map(|_| values.pop().unwrap()),
                            },
                            _ => unreachable!(),
                        };
                        values.push(RVal::Obj(self.alloc_obj(obj, span)?));
                    }
                }
            }
            Ok(values.pop().unwrap())
        })();
        // 子の途中で失敗した場合、まだ親へ移していない値も回収する。
        for v in values {
            self.release_val(v);
        }
        result
    }

    pub fn export(&self, value: RVal) -> Value {
        match value {
            RVal::Void => return Value::Void,
            RVal::Int(n) => return Value::Int(n),
            RVal::Float(n) => return Value::Float(n),
            RVal::Bool(b) => return Value::Bool(b),
            RVal::Char(c) => return Value::Char(c),
            RVal::None => return Value::None,
            RVal::EmptyList => return Value::empty_list(),
            RVal::Builtin(b) => return Value::Builtin(b.name()),
            _ => {}
        }
        self.export_shallow(value, 0)
    }

    #[inline(never)]
    pub(super) fn export_deep(&self, value: RVal) -> Value {
        enum Task {
            Visit(RVal),
            Finish(ObjId, usize),
        }
        let mut tasks = crate::work::Work::<_, 16>::with(Task::Visit(value));
        let mut values = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Visit(v) => {
                    let scalar = match v {
                        RVal::Void => Some(Value::Void),
                        RVal::Int(n) => Some(Value::Int(n)),
                        RVal::Float(n) => Some(Value::Float(n)),
                        RVal::Bool(b) => Some(Value::Bool(b)),
                        RVal::Char(c) => Some(Value::Char(c)),
                        RVal::None => Some(Value::None),
                        RVal::EmptyList => Some(Value::empty_list()),
                        RVal::Builtin(b) => Some(Value::Builtin(b.name())),
                        RVal::Obj(id) => match self.get(id) {
                            Obj::Str(s) => Some(Value::str(s.as_ref())),
                            Obj::Closure { proto, .. } => {
                                Some(Value::Builtin(if proto.name.is_some() {
                                    "<defn>"
                                } else {
                                    "<fn>"
                                }))
                            }
                            Obj::Module { .. } => Some(Value::Builtin("<module>")),
                            obj => {
                                tasks.push(Task::Finish(id, values.len()));
                                match obj {
                                    Obj::Cons { head, tail, .. } => {
                                        tasks.push(Task::Visit(*tail));
                                        tasks.push(Task::Visit(*head));
                                    }
                                    Obj::Array(xs) | Obj::Tuple(xs) => {
                                        tasks.extend(xs.iter().rev().copied().map(Task::Visit))
                                    }
                                    Obj::Struct { fields, .. } => tasks
                                        .extend(fields.values().rev().copied().map(Task::Visit)),
                                    Obj::Some(v) | Obj::Ok(v) | Obj::Err(v) => {
                                        tasks.push(Task::Visit(*v))
                                    }
                                    Obj::Variant { data, .. } => {
                                        if let Some(v) = data {
                                            tasks.push(Task::Visit(*v));
                                        }
                                    }
                                    _ => unreachable!(),
                                }
                                None
                            }
                        },
                    };
                    if let Some(v) = scalar {
                        values.push(v);
                    }
                }
                Task::Finish(id, base) => {
                    let v = match self.get(id) {
                        Obj::Cons { .. } => {
                            let tail = values.pop().unwrap();
                            let head = values.pop().unwrap();
                            let Value::List(xs) = &tail else {
                                unreachable!()
                            };
                            Value::cons(head, xs.clone())
                        }
                        Obj::Array(_) => Value::array(values.drain(base..).collect()),
                        Obj::Tuple(_) => Value::tuple(values.drain(base..).collect()),
                        Obj::Struct { name, fields } => Value::structure(
                            name.as_ref(),
                            fields
                                .keys()
                                .map(|k| k.to_string())
                                .zip(values.drain(base..))
                                .collect(),
                        ),
                        Obj::Some(_) => Value::some(values.pop().unwrap()),
                        Obj::Ok(_) => Value::ok(values.pop().unwrap()),
                        Obj::Err(_) => Value::err(values.pop().unwrap()),
                        Obj::Variant {
                            type_name,
                            variant,
                            data,
                        } => Value::variant(
                            type_name.as_ref(),
                            variant.as_ref(),
                            data.map(|_| values.pop().unwrap()),
                        ),
                        _ => unreachable!(),
                    };
                    values.push(v);
                }
            }
        }
        values.pop().unwrap()
    }
}
