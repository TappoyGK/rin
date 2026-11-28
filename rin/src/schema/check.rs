//! 値の深さに依存しないスキーマ検証。パスは 1 本の文字列を伸縮して保持する。
use super::{Schema, TypeExpr};
use crate::{Error, Span, Value};
use std::collections::BTreeMap;

enum Segment<'a> {
    None,
    Field(&'a str),
    Index(usize),
    Tuple(usize),
}
impl Segment<'_> {
    fn append(&self, path: &mut String) {
        use std::fmt::Write;
        match self {
            Self::None => {}
            Self::Field(s) => {
                path.push('.');
                path.push_str(s);
            }
            Self::Index(i) => {
                write!(path, "[{i}]").unwrap();
            }
            Self::Tuple(i) => {
                write!(path, ".{i}").unwrap();
            }
        }
    }
}

enum Task<'a> {
    Check(&'a Value, &'a TypeExpr, usize, Segment<'a>),
    List(crate::value::ListIter<'a>, &'a TypeExpr, usize, usize),
    Field(
        &'a BTreeMap<String, Value>,
        &'a str,
        &'a str,
        &'a TypeExpr,
        usize,
    ),
    Extra(
        &'a BTreeMap<String, Value>,
        &'a [(String, TypeExpr)],
        &'a str,
        usize,
    ),
}

pub(super) fn check(schema: &Schema, value: &Value) -> Result<(), Error> {
    let mut path = String::from("$");
    let mut work =
        crate::work::Work::<_, 8>::with(Task::Check(value, &schema.root, 1, Segment::None));
    while let Some(task) = work.pop() {
        let (value, mut ty) = match task {
            Task::Check(value, ty, base, suffix) => {
                path.truncate(base);
                suffix.append(&mut path);
                (value, ty)
            }
            Task::List(mut items, ty, base, i) => {
                if let Some(value) = items.next() {
                    work.push(Task::List(items, ty, base, i + 1));
                    work.push(Task::Check(value, ty, base, Segment::Index(i)));
                }
                continue;
            }
            Task::Field(fields, name, field, ty, base) => {
                path.truncate(base);
                let Some(v) = fields.get(field) else {
                    return Err(Error::runtime(
                        "schema.struct",
                        Span::default(),
                        format!("{path}: struct {name} is missing field '{field}'"),
                    ));
                };
                path.push('.');
                path.push_str(field);
                (v, ty)
            }
            Task::Extra(actual, expected, name, base) => {
                path.truncate(base);
                for key in actual.keys() {
                    if !expected.iter().any(|(k, _)| k == key) {
                        return Err(Error::runtime(
                            "schema.struct",
                            Span::default(),
                            format!("{path}: unexpected field '{key}' on struct {name}"),
                        ));
                    }
                }
                continue;
            }
        };
        // 値を消費しない参照だけの循環では、作業スタックを回し続けない。
        let mut refs = 0;
        while let TypeExpr::Ref(name) = ty {
            refs += 1;
            if refs > schema.defs.len() {
                return Err(Error::runtime(
                    "schema.productive",
                    Span::default(),
                    format!("{path}: cyclic schema reference '{name}'"),
                ));
            }
            ty = schema.defs.get(name).ok_or_else(|| {
                Error::runtime(
                    "schema.reference",
                    Span::default(),
                    format!("unknown schema '{name}'"),
                )
            })?;
        }
        let base = path.len();
        match ty {
            TypeExpr::Datum => match value {
                Value::Int(_)
                | Value::Float(_)
                | Value::Bool(_)
                | Value::Char(_)
                | Value::Text(_)
                | Value::Ident(_)
                | Value::None => {}
                Value::List(xs) => {
                    work.push(Task::List(xs.iter(), ty, base, 0));
                }
                _ => return mismatch("datum", value.type_name(), &path),
            },
            TypeExpr::Void => expect_match(matches!(value, Value::Void), "void", value, &path)?,
            TypeExpr::I64 => expect_match(matches!(value, Value::Int(_)), "i64", value, &path)?,
            TypeExpr::F64 => expect_match(matches!(value, Value::Float(_)), "f64", value, &path)?,
            TypeExpr::Bool => expect_match(matches!(value, Value::Bool(_)), "bool", value, &path)?,
            TypeExpr::Char => expect_match(matches!(value, Value::Char(_)), "char", value, &path)?,
            TypeExpr::Text => expect_match(matches!(value, Value::Text(_)), "text", value, &path)?,
            TypeExpr::Ident => {
                expect_match(matches!(value, Value::Ident(_)), "ident", value, &path)?
            }
            TypeExpr::List(ty) => {
                let Value::List(xs) = value else {
                    return mismatch("list", value.type_name(), &path);
                };
                work.push(Task::List(xs.iter(), ty, base, 0));
            }
            TypeExpr::Array(ty) => {
                let Value::Array(xs) = value else {
                    return mismatch("array", value.type_name(), &path);
                };
                for (i, x) in xs.iter().enumerate().rev() {
                    work.push(Task::Check(x, ty, base, Segment::Index(i)));
                }
            }
            TypeExpr::Tuple(types) => {
                let Value::Tuple(xs) = value else {
                    return mismatch("tuple", value.type_name(), &path);
                };
                if xs.len() != types.len() {
                    return Err(Error::runtime(
                        "schema.tuple",
                        Span::default(),
                        format!(
                            "{path}: expected tuple of {} elements, got {}",
                            types.len(),
                            xs.len()
                        ),
                    ));
                }
                for (i, (x, ty)) in xs.iter().zip(types).enumerate().rev() {
                    work.push(Task::Check(x, ty, base, Segment::Tuple(i)));
                }
            }
            TypeExpr::Option(ty) => match value {
                Value::None => {}
                Value::Some(x) => work.push(Task::Check(x, ty, base, Segment::Field("some"))),
                other => return mismatch("option", other.type_name(), &path),
            },
            TypeExpr::Result(ok, err) => match value {
                Value::Ok(x) => work.push(Task::Check(x, ok, base, Segment::Field("ok"))),
                Value::Err(x) => work.push(Task::Check(x, err, base, Segment::Field("err"))),
                other => return mismatch("result", other.type_name(), &path),
            },
            TypeExpr::Struct { name, fields, open } => {
                let Value::Struct {
                    name: actual_name,
                    fields: actual,
                } = value
                else {
                    return mismatch(name, value.type_name(), &path);
                };
                if actual_name.as_str() != name {
                    return mismatch(name, actual_name, &path);
                }
                if !open {
                    work.push(Task::Extra(actual, fields, name, base));
                }
                for (field, ty) in fields.iter().rev() {
                    work.push(Task::Field(actual, name, field, ty, base));
                }
            }
            TypeExpr::Variant { name, cases } => {
                let Value::Variant {
                    type_name,
                    variant,
                    data,
                } = value
                else {
                    return mismatch(name, value.type_name(), &path);
                };
                if type_name.as_str() != name {
                    return mismatch(name, type_name, &path);
                }
                let Some((_, payload)) = cases.iter().find(|(v, _)| v == variant.as_str()) else {
                    return Err(Error::runtime(
                        "schema.variant",
                        Span::default(),
                        format!("{path}: {name} has no variant '{variant}'"),
                    ));
                };
                match (payload, data) {
                    (None, None) => {}
                    (Some(ty), Some(x)) => {
                        work.push(Task::Check(x, ty, base, Segment::Field(variant)))
                    }
                    (None, Some(_)) => {
                        return Err(Error::runtime(
                            "schema.variant",
                            Span::default(),
                            format!("{path}: {name}.{variant} takes no payload"),
                        ));
                    }
                    (Some(_), None) => {
                        return Err(Error::runtime(
                            "schema.variant",
                            Span::default(),
                            format!("{path}: {name}.{variant} requires a payload"),
                        ));
                    }
                }
            }
            TypeExpr::Ref(_) => unreachable!(),
        }
    }
    Ok(())
}

fn expect_match(ok: bool, expected: &str, actual: &Value, path: &str) -> Result<(), Error> {
    if ok {
        Ok(())
    } else {
        mismatch(expected, actual.type_name(), path)
    }
}

fn mismatch(expected: &str, actual: &str, path: &str) -> Result<(), Error> {
    Err(Error::runtime(
        "schema.type",
        Span::default(),
        format!("{path}: expected {expected}, got {actual}"),
    ))
}
