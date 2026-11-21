//! 固定深度までの直接変換。上限以降は反復経路だけで処理する。
use super::*;
impl Pool<'_> {
    pub(super) fn export_shallow(&self, val: RVal, depth: usize) -> Value {
        if depth == 32
            && matches!(val, RVal::Obj(id) if !matches!(self.get(id), Obj::Str(_) | Obj::Closure { .. } | Obj::Module { .. }))
        {
            return self.export_deep(val);
        }
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
                        items.push(self.export_shallow(*head, depth + 1));
                        cur = *tail;
                    }
                    Value::list(items)
                }
                Obj::Str(s) => Value::str(s.as_ref()),
                Obj::Array(xs) => Value::array(
                    xs.iter()
                        .map(|v| self.export_shallow(*v, depth + 1))
                        .collect(),
                ),
                Obj::Tuple(xs) => Value::tuple(
                    xs.iter()
                        .map(|v| self.export_shallow(*v, depth + 1))
                        .collect(),
                ),
                Obj::Struct { name, fields } => {
                    // 少数フィールドでは中間 Vec とソートを省く。
                    // 大きな map は標準の一括構築で木の逐次更新を避ける。
                    let map = if fields.len() <= 4 {
                        let mut map = BTreeMap::new();
                        for (k, v) in fields {
                            map.insert(k.to_string(), self.export_shallow(*v, depth + 1));
                        }
                        map
                    } else {
                        fields
                            .iter()
                            .map(|(k, v)| (k.to_string(), self.export_shallow(*v, depth + 1)))
                            .collect()
                    };
                    Value::structure(name.as_ref(), map)
                }
                Obj::Variant {
                    type_name,
                    variant,
                    data,
                } => Value::variant(
                    type_name.as_ref(),
                    variant.as_ref(),
                    data.map(|d| self.export_shallow(d, depth + 1)),
                ),
                Obj::Some(v) => Value::Some(std::rc::Rc::new(self.export_shallow(*v, depth + 1))),
                Obj::Ok(v) => Value::Ok(std::rc::Rc::new(self.export_shallow(*v, depth + 1))),
                Obj::Err(v) => Value::Err(std::rc::Rc::new(self.export_shallow(*v, depth + 1))),
                Obj::Closure { proto, .. } => Value::Builtin(match proto.name.as_deref() {
                    Some(_) => "<defn>",
                    None => "<fn>",
                }),
                Obj::Module { .. } => Value::Builtin("<module>"),
            },
        }
    }

    pub(super) fn import_shallow(
        &mut self,
        val: &Value,
        span: Span,
        depth: usize,
    ) -> Result<RVal, Error> {
        if depth == 32
            && matches!(
                val,
                Value::List(_)
                    | Value::Array(_)
                    | Value::Tuple(_)
                    | Value::Struct { .. }
                    | Value::Variant { .. }
                    | Value::Some(_)
                    | Value::Ok(_)
                    | Value::Err(_)
            )
        {
            return self.import_deep(val, span);
        }
        match val {
            Value::Void => Ok(RVal::Void),
            Value::Int(n) => Ok(RVal::Int(*n)),
            Value::Float(n) => Ok(RVal::Float(*n)),
            Value::Bool(b) => Ok(RVal::Bool(*b)),
            Value::Char(c) => Ok(RVal::Char(*c)),
            Value::None => Ok(RVal::None),
            Value::Builtin(name) => BuiltinId::ALL
                .iter()
                .copied()
                .find(|b| b.name() == *name)
                .map(RVal::Builtin)
                .ok_or_else(|| Error::runtime(span, format!("cannot import '{name}'"))),
            Value::Str(s) => self
                .alloc_obj(Obj::Str(Rc::from(s.as_str())), span)
                .map(RVal::Obj),
            Value::List(xs) => {
                let items: Vec<_> = xs.iter().collect();
                let mut acc = RVal::EmptyList;
                for (index, x) in items.into_iter().rev().enumerate() {
                    let head = match self.import_shallow(x, span, depth + 1) {
                        Ok(v) => v,
                        Err(e) => {
                            self.release_val(acc);
                            return Err(e);
                        }
                    };
                    let len = index + 1;
                    acc = RVal::Obj(self.alloc_obj(
                        Obj::Cons {
                            head,
                            tail: acc,
                            len,
                        },
                        span,
                    )?);
                }
                Ok(acc)
            }
            Value::Array(xs) | Value::Tuple(xs) => {
                let mut values = Vec::with_capacity(xs.len());
                for x in xs.iter() {
                    match self.import_shallow(x, span, depth + 1) {
                        Ok(v) => values.push(v),
                        Err(e) => {
                            for v in values {
                                self.release_val(v);
                            }
                            return Err(e);
                        }
                    }
                }
                let obj = if matches!(val, Value::Array(_)) {
                    Obj::Array(values)
                } else {
                    Obj::Tuple(values)
                };
                self.alloc_obj(obj, span).map(RVal::Obj)
            }
            Value::Struct { name, fields } => {
                let mut values = BTreeMap::new();
                for (key, x) in fields.iter() {
                    match self.import_shallow(x, span, depth + 1) {
                        Ok(v) => {
                            values.insert(Rc::from(key.as_str()), v);
                        }
                        Err(e) => {
                            for v in values.into_values() {
                                self.release_val(v);
                            }
                            return Err(e);
                        }
                    }
                }
                self.alloc_obj(
                    Obj::Struct {
                        name: Rc::from(name.as_str()),
                        fields: values,
                    },
                    span,
                )
                .map(RVal::Obj)
            }
            Value::Variant {
                type_name,
                variant,
                data,
            } => {
                let data = match data {
                    Some(x) => Some(self.import_shallow(x, span, depth + 1)?),
                    None => None,
                };
                self.alloc_obj(
                    Obj::Variant {
                        type_name: Rc::from(type_name.as_str()),
                        variant: Rc::from(variant.as_str()),
                        data,
                    },
                    span,
                )
                .map(RVal::Obj)
            }
            Value::Some(x) | Value::Ok(x) | Value::Err(x) => {
                let x = self.import_shallow(x, span, depth + 1)?;
                let obj = match val {
                    Value::Some(_) => Obj::Some(x),
                    Value::Ok(_) => Obj::Ok(x),
                    _ => Obj::Err(x),
                };
                self.alloc_obj(obj, span).map(RVal::Obj)
            }
        }
    }
}
