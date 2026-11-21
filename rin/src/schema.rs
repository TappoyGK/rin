//! Rin の値構造を検証するスキーマ DSL の構文解析と照合を実装する。

use std::collections::BTreeMap;
use std::fmt;

use crate::error::{Error, Span};
use crate::lexer::{Token, TokenKind, tokenize};
use crate::parser::MAX_SYNTAX_NESTING;
use crate::value::Value;

pub enum TypeExpr {
    Any,
    Void,
    I64,
    F64,
    Bool,
    Char,
    String,
    List(Box<TypeExpr>),
    Array(Box<TypeExpr>),
    Tuple(Vec<TypeExpr>),
    Option(Box<TypeExpr>),
    Result(Box<TypeExpr>, Box<TypeExpr>),
    Struct {
        name: String,
        fields: Vec<(String, TypeExpr)>,
        open: bool,
    },
    Variant {
        name: String,
        cases: Vec<(String, Option<TypeExpr>)>,
    },
    Ref(String),
}

#[inline(always)]
fn has_type_children(ty: &TypeExpr) -> bool {
    !matches!(
        ty,
        TypeExpr::Any
            | TypeExpr::Void
            | TypeExpr::I64
            | TypeExpr::F64
            | TypeExpr::Bool
            | TypeExpr::Char
            | TypeExpr::String
            | TypeExpr::Ref(_)
    )
}

const SHALLOW_TYPE_DEPTH: usize = 32;

#[derive(Clone, Copy)]
enum UnaryTypeKind {
    List,
    Array,
    Option,
}

#[inline(always)]
fn push_type_if_non_leaf(ty: TypeExpr, next: &mut Option<TypeExpr>, work: &mut Vec<TypeExpr>) {
    if has_type_children(&ty) {
        if next.is_none() {
            *next = Some(ty);
        } else {
            work.push(ty);
        }
    }
}

impl Drop for TypeExpr {
    fn drop(&mut self) {
        if !has_type_children(self) {
            return;
        }
        let mut next = None;
        let mut work = Vec::new();
        take_type_children(self, &mut next, &mut work);
        loop {
            let mut cur = match next.take() {
                Some(c) => c,
                None => match work.pop() {
                    Some(c) => c,
                    None => break,
                },
            };
            take_type_children(&mut cur, &mut next, &mut work);
        }
    }
}

fn take_type_children(ty: &mut TypeExpr, next: &mut Option<TypeExpr>, work: &mut Vec<TypeExpr>) {
    match ty {
        TypeExpr::Any
        | TypeExpr::Void
        | TypeExpr::I64
        | TypeExpr::F64
        | TypeExpr::Bool
        | TypeExpr::Char
        | TypeExpr::String
        | TypeExpr::Ref(_) => {}
        TypeExpr::List(t) | TypeExpr::Array(t) | TypeExpr::Option(t) => {
            push_type_if_non_leaf(std::mem::replace(&mut **t, TypeExpr::Any), next, work);
        }
        TypeExpr::Result(a, b) => {
            push_type_if_non_leaf(std::mem::replace(&mut **a, TypeExpr::Any), next, work);
            push_type_if_non_leaf(std::mem::replace(&mut **b, TypeExpr::Any), next, work);
        }
        TypeExpr::Tuple(elems) => {
            for t in std::mem::take(elems) {
                push_type_if_non_leaf(t, next, work);
            }
        }
        TypeExpr::Struct { fields, .. } => {
            for (_, t) in std::mem::take(fields) {
                push_type_if_non_leaf(t, next, work);
            }
        }
        TypeExpr::Variant { cases, .. } => {
            for (_, opt) in std::mem::take(cases) {
                if let Some(t) = opt {
                    push_type_if_non_leaf(t, next, work);
                }
            }
        }
    }
}

impl TypeExpr {
    fn clone_shallow(&self, depth: usize) -> Self {
        if depth >= SHALLOW_TYPE_DEPTH {
            return self.clone_iter();
        }
        match self {
            TypeExpr::Any => TypeExpr::Any,
            TypeExpr::Void => TypeExpr::Void,
            TypeExpr::I64 => TypeExpr::I64,
            TypeExpr::F64 => TypeExpr::F64,
            TypeExpr::Bool => TypeExpr::Bool,
            TypeExpr::Char => TypeExpr::Char,
            TypeExpr::String => TypeExpr::String,
            TypeExpr::Ref(s) => TypeExpr::Ref(s.clone()),
            TypeExpr::List(t) => TypeExpr::List(Box::new(t.clone_shallow(depth + 1))),
            TypeExpr::Array(t) => TypeExpr::Array(Box::new(t.clone_shallow(depth + 1))),
            TypeExpr::Option(t) => TypeExpr::Option(Box::new(t.clone_shallow(depth + 1))),
            TypeExpr::Result(a, b) => TypeExpr::Result(
                Box::new(a.clone_shallow(depth + 1)),
                Box::new(b.clone_shallow(depth + 1)),
            ),
            TypeExpr::Tuple(elems) => {
                TypeExpr::Tuple(elems.iter().map(|t| t.clone_shallow(depth + 1)).collect())
            }
            TypeExpr::Struct { name, fields, open } => TypeExpr::Struct {
                name: name.clone(),
                fields: fields
                    .iter()
                    .map(|(k, t)| (k.clone(), t.clone_shallow(depth + 1)))
                    .collect(),
                open: *open,
            },
            TypeExpr::Variant { name, cases } => TypeExpr::Variant {
                name: name.clone(),
                cases: cases
                    .iter()
                    .map(|(k, opt)| (k.clone(), opt.as_ref().map(|t| t.clone_shallow(depth + 1))))
                    .collect(),
            },
        }
    }

    #[cold]
    #[inline(never)]
    fn clone_iter(&self) -> Self {
        enum CloneFrame<'a> {
            Unary(UnaryTypeKind),
            ResultFirst(&'a TypeExpr),
            ResultSecond(Box<TypeExpr>),
            Tuple {
                rest: &'a [TypeExpr],
                done: Vec<TypeExpr>,
            },
            Struct {
                name: &'a str,
                open: bool,
                rest: &'a [(String, TypeExpr)],
                cur_key: &'a str,
                done: Vec<(String, TypeExpr)>,
            },
            Variant {
                name: &'a str,
                rest: &'a [(String, Option<TypeExpr>)],
                cur_key: &'a str,
                done: Vec<(String, Option<TypeExpr>)>,
            },
        }
        enum CloneNext<'a> {
            Start(&'a TypeExpr),
            Have(TypeExpr),
        }

        let mut frames: Vec<CloneFrame<'_>> = Vec::new();
        let mut next = CloneNext::Start(self);

        'clone_loop: loop {
            match next {
                CloneNext::Start(cur) => match cur {
                    TypeExpr::Any => next = CloneNext::Have(TypeExpr::Any),
                    TypeExpr::Void => next = CloneNext::Have(TypeExpr::Void),
                    TypeExpr::I64 => next = CloneNext::Have(TypeExpr::I64),
                    TypeExpr::F64 => next = CloneNext::Have(TypeExpr::F64),
                    TypeExpr::Bool => next = CloneNext::Have(TypeExpr::Bool),
                    TypeExpr::Char => next = CloneNext::Have(TypeExpr::Char),
                    TypeExpr::String => next = CloneNext::Have(TypeExpr::String),
                    TypeExpr::Ref(s) => next = CloneNext::Have(TypeExpr::Ref(s.clone())),
                    TypeExpr::List(t) => {
                        frames.push(CloneFrame::Unary(UnaryTypeKind::List));
                        next = CloneNext::Start(t);
                    }
                    TypeExpr::Array(t) => {
                        frames.push(CloneFrame::Unary(UnaryTypeKind::Array));
                        next = CloneNext::Start(t);
                    }
                    TypeExpr::Option(t) => {
                        frames.push(CloneFrame::Unary(UnaryTypeKind::Option));
                        next = CloneNext::Start(t);
                    }
                    TypeExpr::Result(a, b) => {
                        frames.push(CloneFrame::ResultFirst(b));
                        next = CloneNext::Start(a);
                    }
                    TypeExpr::Tuple(elems) => {
                        if elems.is_empty() {
                            next = CloneNext::Have(TypeExpr::Tuple(Vec::new()));
                        } else {
                            frames.push(CloneFrame::Tuple {
                                rest: &elems[1..],
                                done: Vec::with_capacity(elems.len()),
                            });
                            next = CloneNext::Start(&elems[0]);
                        }
                    }
                    TypeExpr::Struct { name, fields, open } => {
                        if fields.is_empty() {
                            next = CloneNext::Have(TypeExpr::Struct {
                                name: name.clone(),
                                fields: Vec::new(),
                                open: *open,
                            });
                        } else {
                            frames.push(CloneFrame::Struct {
                                name,
                                open: *open,
                                rest: &fields[1..],
                                cur_key: &fields[0].0,
                                done: Vec::with_capacity(fields.len()),
                            });
                            next = CloneNext::Start(&fields[0].1);
                        }
                    }
                    TypeExpr::Variant { name, cases } => {
                        let mut done = Vec::with_capacity(cases.len());
                        for (idx, (vname, opt)) in cases.iter().enumerate() {
                            if let Some(pty) = opt {
                                frames.push(CloneFrame::Variant {
                                    name,
                                    rest: &cases[idx + 1..],
                                    cur_key: vname,
                                    done,
                                });
                                next = CloneNext::Start(pty);
                                continue 'clone_loop;
                            }
                            done.push((vname.clone(), None));
                        }
                        next = CloneNext::Have(TypeExpr::Variant {
                            name: name.clone(),
                            cases: done,
                        });
                    }
                },
                CloneNext::Have(val) => {
                    let Some(frame) = frames.pop() else {
                        return val;
                    };
                    match frame {
                        CloneFrame::Unary(UnaryTypeKind::List) => {
                            next = CloneNext::Have(TypeExpr::List(Box::new(val)));
                        }
                        CloneFrame::Unary(UnaryTypeKind::Array) => {
                            next = CloneNext::Have(TypeExpr::Array(Box::new(val)));
                        }
                        CloneFrame::Unary(UnaryTypeKind::Option) => {
                            next = CloneNext::Have(TypeExpr::Option(Box::new(val)));
                        }
                        CloneFrame::ResultFirst(b) => {
                            frames.push(CloneFrame::ResultSecond(Box::new(val)));
                            next = CloneNext::Start(b);
                        }
                        CloneFrame::ResultSecond(ok) => {
                            next = CloneNext::Have(TypeExpr::Result(ok, Box::new(val)));
                        }
                        CloneFrame::Tuple { rest, mut done } => {
                            done.push(val);
                            if rest.is_empty() {
                                next = CloneNext::Have(TypeExpr::Tuple(done));
                            } else {
                                frames.push(CloneFrame::Tuple {
                                    rest: &rest[1..],
                                    done,
                                });
                                next = CloneNext::Start(&rest[0]);
                            }
                        }
                        CloneFrame::Struct {
                            name,
                            open,
                            rest,
                            cur_key,
                            mut done,
                        } => {
                            done.push((cur_key.to_string(), val));
                            if rest.is_empty() {
                                next = CloneNext::Have(TypeExpr::Struct {
                                    name: name.to_string(),
                                    fields: done,
                                    open,
                                });
                            } else {
                                frames.push(CloneFrame::Struct {
                                    name,
                                    open,
                                    rest: &rest[1..],
                                    cur_key: &rest[0].0,
                                    done,
                                });
                                next = CloneNext::Start(&rest[0].1);
                            }
                        }
                        CloneFrame::Variant {
                            name,
                            rest,
                            cur_key,
                            mut done,
                        } => {
                            done.push((cur_key.to_string(), Some(val)));
                            for (idx, (vname, opt)) in rest.iter().enumerate() {
                                if let Some(pty) = opt {
                                    frames.push(CloneFrame::Variant {
                                        name,
                                        rest: &rest[idx + 1..],
                                        cur_key: vname,
                                        done,
                                    });
                                    next = CloneNext::Start(pty);
                                    continue 'clone_loop;
                                }
                                done.push((vname.clone(), None));
                            }
                            next = CloneNext::Have(TypeExpr::Variant {
                                name: name.to_string(),
                                cases: done,
                            });
                        }
                    }
                }
            }
        }
    }
}

impl Clone for TypeExpr {
    #[inline]
    fn clone(&self) -> Self {
        self.clone_shallow(0)
    }
}

impl TypeExpr {
    fn fmt_shallow(&self, f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
        if depth >= SHALLOW_TYPE_DEPTH {
            return self.fmt_iter(f);
        }
        struct ShallowTy<'a>(&'a TypeExpr, usize);
        impl fmt::Debug for ShallowTy<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt_shallow(f, self.1)
            }
        }
        struct ShallowList<'a>(&'a [TypeExpr], usize);
        impl fmt::Debug for ShallowList<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_list()
                    .entries(self.0.iter().map(|t| ShallowTy(t, self.1)))
                    .finish()
            }
        }
        struct ShallowFields<'a>(&'a [(String, TypeExpr)], usize);
        impl fmt::Debug for ShallowFields<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_list()
                    .entries(self.0.iter().map(|(k, t)| (k, ShallowTy(t, self.1))))
                    .finish()
            }
        }
        struct ShallowOpt<'a>(&'a Option<TypeExpr>, usize);
        impl fmt::Debug for ShallowOpt<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self.0 {
                    Some(t) => f.debug_tuple("Some").field(&ShallowTy(t, self.1)).finish(),
                    None => f.write_str("None"),
                }
            }
        }
        struct ShallowCases<'a>(&'a [(String, Option<TypeExpr>)], usize);
        impl fmt::Debug for ShallowCases<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_list()
                    .entries(self.0.iter().map(|(k, o)| (k, ShallowOpt(o, self.1))))
                    .finish()
            }
        }

        match self {
            TypeExpr::Any => f.write_str("Any"),
            TypeExpr::Void => f.write_str("Void"),
            TypeExpr::I64 => f.write_str("I64"),
            TypeExpr::F64 => f.write_str("F64"),
            TypeExpr::Bool => f.write_str("Bool"),
            TypeExpr::Char => f.write_str("Char"),
            TypeExpr::String => f.write_str("String"),
            TypeExpr::Ref(s) => f.debug_tuple("Ref").field(s).finish(),
            TypeExpr::List(t) => f
                .debug_tuple("List")
                .field(&ShallowTy(t, depth + 1))
                .finish(),
            TypeExpr::Array(t) => f
                .debug_tuple("Array")
                .field(&ShallowTy(t, depth + 1))
                .finish(),
            TypeExpr::Option(t) => f
                .debug_tuple("Option")
                .field(&ShallowTy(t, depth + 1))
                .finish(),
            TypeExpr::Result(a, b) => f
                .debug_tuple("Result")
                .field(&ShallowTy(a, depth + 1))
                .field(&ShallowTy(b, depth + 1))
                .finish(),
            TypeExpr::Tuple(elems) => f
                .debug_tuple("Tuple")
                .field(&ShallowList(elems, depth + 1))
                .finish(),
            TypeExpr::Struct { name, fields, open } => f
                .debug_struct("Struct")
                .field("name", name)
                .field("fields", &ShallowFields(fields, depth + 1))
                .field("open", open)
                .finish(),
            TypeExpr::Variant { name, cases } => f
                .debug_struct("Variant")
                .field("name", name)
                .field("cases", &ShallowCases(cases, depth + 1))
                .finish(),
        }
    }

    #[cold]
    #[inline(never)]
    fn fmt_iter(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        enum DebugItem<'a> {
            Lit(&'static str),
            Quoted(&'a str),
            Bool(bool),
            Ty(&'a TypeExpr),
        }
        let mut stack = Vec::with_capacity(64);
        stack.push(DebugItem::Ty(self));
        while let Some(item) = stack.pop() {
            match item {
                DebugItem::Lit(s) => f.write_str(s)?,
                DebugItem::Quoted(s) => write!(f, "{s:?}")?,
                DebugItem::Bool(b) => write!(f, "{b:?}")?,
                DebugItem::Ty(cur) => match cur {
                    TypeExpr::Any => f.write_str("Any")?,
                    TypeExpr::Void => f.write_str("Void")?,
                    TypeExpr::I64 => f.write_str("I64")?,
                    TypeExpr::F64 => f.write_str("F64")?,
                    TypeExpr::Bool => f.write_str("Bool")?,
                    TypeExpr::Char => f.write_str("Char")?,
                    TypeExpr::String => f.write_str("String")?,
                    TypeExpr::Ref(s) => {
                        f.write_str("Ref(")?;
                        write!(f, "{s:?})")?;
                    }
                    TypeExpr::List(t) => {
                        f.write_str("List(")?;
                        stack.push(DebugItem::Lit(")"));
                        stack.push(DebugItem::Ty(t));
                    }
                    TypeExpr::Array(t) => {
                        f.write_str("Array(")?;
                        stack.push(DebugItem::Lit(")"));
                        stack.push(DebugItem::Ty(t));
                    }
                    TypeExpr::Option(t) => {
                        f.write_str("Option(")?;
                        stack.push(DebugItem::Lit(")"));
                        stack.push(DebugItem::Ty(t));
                    }
                    TypeExpr::Result(a, b) => {
                        f.write_str("Result(")?;
                        stack.push(DebugItem::Lit(")"));
                        stack.push(DebugItem::Ty(b));
                        stack.push(DebugItem::Lit(", "));
                        stack.push(DebugItem::Ty(a));
                    }
                    TypeExpr::Tuple(elems) => {
                        f.write_str("Tuple([")?;
                        stack.push(DebugItem::Lit("])"));
                        for (i, t) in elems.iter().enumerate().rev() {
                            stack.push(DebugItem::Ty(t));
                            if i > 0 {
                                stack.push(DebugItem::Lit(", "));
                            }
                        }
                    }
                    TypeExpr::Struct { name, fields, open } => {
                        f.write_str("Struct { name: ")?;
                        write!(f, "{name:?}")?;
                        f.write_str(", fields: [")?;
                        stack.push(DebugItem::Lit(" }"));
                        stack.push(DebugItem::Bool(*open));
                        stack.push(DebugItem::Lit("], open: "));
                        for (i, (k, t)) in fields.iter().enumerate().rev() {
                            stack.push(DebugItem::Lit(")"));
                            stack.push(DebugItem::Ty(t));
                            stack.push(DebugItem::Lit(", "));
                            stack.push(DebugItem::Quoted(k));
                            stack.push(DebugItem::Lit("("));
                            if i > 0 {
                                stack.push(DebugItem::Lit(", "));
                            }
                        }
                    }
                    TypeExpr::Variant { name, cases } => {
                        f.write_str("Variant { name: ")?;
                        write!(f, "{name:?}")?;
                        f.write_str(", cases: [")?;
                        stack.push(DebugItem::Lit("] }"));
                        for (i, (k, opt)) in cases.iter().enumerate().rev() {
                            if let Some(t) = opt {
                                stack.push(DebugItem::Lit("))"));
                                stack.push(DebugItem::Ty(t));
                                stack.push(DebugItem::Lit(", Some("));
                            } else {
                                stack.push(DebugItem::Lit(", None)"));
                            }
                            stack.push(DebugItem::Quoted(k));
                            stack.push(DebugItem::Lit("("));
                            if i > 0 {
                                stack.push(DebugItem::Lit(", "));
                            }
                        }
                    }
                },
            }
        }
        Ok(())
    }
}

impl fmt::Debug for TypeExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_shallow(f, 0)
    }
}

/// `Value` の構造を検証するコンパイル済み Rin スキーマ。
#[derive(Debug, Clone)]
pub struct Schema {
    defs: BTreeMap<String, TypeExpr>,
    root: TypeExpr,
}

impl Schema {
    /// 0 個以上の `(defschema Name TypeExpr)` 宣言と末尾のルート `TypeExpr` を解析する。
    pub fn parse(source: &str) -> Result<Self, Error> {
        let tokens = tokenize(source)?;
        let mut p = SchemaParser {
            tokens,
            pos: 0,
            frames: Vec::new(),
        };
        let mut defs = BTreeMap::new();
        let mut root = None;

        while !p.at_eof() {
            if p.peek_is_defschema() {
                let (name, ty) = p.parse_defschema()?;
                defs.insert(name, ty);
            } else {
                let ty = p.parse_type()?;
                if !p.at_eof() {
                    return Err(Error::parse(
                        p.peek().span,
                        "expected end of schema after root type expression",
                    ));
                }
                root = Some(ty);
                break;
            }
        }

        let root = root.ok_or_else(|| {
            Error::parse(
                Span::new(1, 1),
                "schema must end with a root type expression",
            )
        })?;
        let schema = Self { defs, root };
        let mut ref_stack = Vec::new();
        schema.validate_refs(&schema.root, &mut ref_stack)?;
        for ty in schema.defs.values() {
            schema.validate_refs(ty, &mut ref_stack)?;
        }
        Ok(schema)
    }

    fn validate_refs<'a>(
        &'a self,
        ty: &'a TypeExpr,
        stack: &mut Vec<&'a TypeExpr>,
    ) -> Result<(), Error> {
        if !has_type_children(ty) {
            if let TypeExpr::Ref(name) = ty
                && !self.defs.contains_key(name)
            {
                return Err(Error::parse(
                    Span::new(1, 1),
                    format!("unknown schema name '{name}'"),
                ));
            }
            return Ok(());
        }
        stack.clear();
        stack.push(ty);
        while let Some(cur) = stack.pop() {
            match cur {
                TypeExpr::Any
                | TypeExpr::Void
                | TypeExpr::I64
                | TypeExpr::F64
                | TypeExpr::Bool
                | TypeExpr::Char
                | TypeExpr::String => {}
                TypeExpr::List(inner) | TypeExpr::Array(inner) | TypeExpr::Option(inner) => {
                    stack.push(inner);
                }
                TypeExpr::Result(ok, err) => {
                    stack.push(ok);
                    stack.push(err);
                }
                TypeExpr::Tuple(items) => {
                    stack.extend(items.iter());
                }
                TypeExpr::Struct { fields, .. } => {
                    for (_, fty) in fields {
                        stack.push(fty);
                    }
                }
                TypeExpr::Variant { cases, .. } => {
                    for (_, payload) in cases {
                        if let Some(pty) = payload {
                            stack.push(pty);
                        }
                    }
                }
                TypeExpr::Ref(name) => {
                    if !self.defs.contains_key(name) {
                        return Err(Error::parse(
                            Span::new(1, 1),
                            format!("unknown schema name '{name}'"),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// `value` がこのスキーマに適合するか検証する。
    pub fn check(&self, value: &Value) -> Result<(), Error> {
        self.check_type(value, &self.root, "$")
    }

    fn check_type(&self, value: &Value, ty: &TypeExpr, path: &str) -> Result<(), Error> {
        match ty {
            TypeExpr::Any => Ok(()),
            TypeExpr::Void => expect_match(matches!(value, Value::Void), "void", value, path),
            TypeExpr::I64 => expect_match(matches!(value, Value::Int(_)), "i64", value, path),
            TypeExpr::F64 => expect_match(matches!(value, Value::Float(_)), "f64", value, path),
            TypeExpr::Bool => expect_match(matches!(value, Value::Bool(_)), "bool", value, path),
            TypeExpr::Char => expect_match(matches!(value, Value::Char(_)), "char", value, path),
            TypeExpr::String => expect_match(matches!(value, Value::Str(_)), "string", value, path),
            TypeExpr::List(elem_ty) => {
                let Value::List(xs) = value else {
                    return mismatch("list", value.type_name(), path);
                };
                for (i, item) in xs.iter().enumerate() {
                    self.check_type(item, elem_ty, &format!("{path}[{i}]"))?;
                }
                Ok(())
            }
            TypeExpr::Array(elem_ty) => {
                let Value::Array(xs) = value else {
                    return mismatch("array", value.type_name(), path);
                };
                for (i, item) in xs.iter().enumerate() {
                    self.check_type(item, elem_ty, &format!("{path}[{i}]"))?;
                }
                Ok(())
            }
            TypeExpr::Tuple(elem_tys) => {
                let Value::Tuple(xs) = value else {
                    return mismatch("tuple", value.type_name(), path);
                };
                if xs.len() != elem_tys.len() {
                    return Err(Error::runtime(
                        Span::default(),
                        format!(
                            "{path}: expected tuple of {} elements, got {}",
                            elem_tys.len(),
                            xs.len()
                        ),
                    ));
                }
                for (i, (item, ety)) in xs.iter().zip(elem_tys.iter()).enumerate() {
                    self.check_type(item, ety, &format!("{path}.{i}"))?;
                }
                Ok(())
            }
            TypeExpr::Option(inner_ty) => match value {
                Value::None => Ok(()),
                Value::Some(inner) => self.check_type(inner, inner_ty, &format!("{path}.some")),
                other => mismatch("option", other.type_name(), path),
            },
            TypeExpr::Result(ok_ty, err_ty) => match value {
                Value::Ok(inner) => self.check_type(inner, ok_ty, &format!("{path}.ok")),
                Value::Err(inner) => self.check_type(inner, err_ty, &format!("{path}.err")),
                other => mismatch("result", other.type_name(), path),
            },
            TypeExpr::Struct { name, fields, open } => {
                let Value::Struct {
                    name: actual_name,
                    fields: actual_fields,
                } = value
                else {
                    return mismatch(name, value.type_name(), path);
                };
                if actual_name.as_str() != name {
                    return mismatch(name, actual_name, path);
                }
                for (fname, fty) in fields {
                    let Some(fval) = actual_fields.get(fname) else {
                        return Err(Error::runtime(
                            Span::default(),
                            format!("{path}: struct {name} is missing field '{fname}'"),
                        ));
                    };
                    self.check_type(fval, fty, &format!("{path}.{fname}"))?;
                }
                if !open {
                    for actual_key in actual_fields.keys() {
                        if !fields.iter().any(|(k, _)| k == actual_key) {
                            return Err(Error::runtime(
                                Span::default(),
                                format!("{path}: unexpected field '{actual_key}' on struct {name}"),
                            ));
                        }
                    }
                }
                Ok(())
            }
            TypeExpr::Variant { name, cases } => {
                let Value::Variant {
                    type_name,
                    variant,
                    data,
                } = value
                else {
                    return mismatch(name, value.type_name(), path);
                };
                if type_name.as_str() != name {
                    return mismatch(name, type_name, path);
                }
                let Some((_, expected_payload)) = cases.iter().find(|(v, _)| v == variant.as_str())
                else {
                    return Err(Error::runtime(
                        Span::default(),
                        format!("{path}: {name} has no variant '{variant}'"),
                    ));
                };
                match (expected_payload, data) {
                    (None, None) => Ok(()),
                    (Some(pty), Some(actual)) => {
                        self.check_type(actual, pty, &format!("{path}.{variant}"))
                    }
                    (None, Some(_)) => Err(Error::runtime(
                        Span::default(),
                        format!("{path}: {name}.{variant} takes no payload"),
                    )),
                    (Some(_), None) => Err(Error::runtime(
                        Span::default(),
                        format!("{path}: {name}.{variant} requires a payload"),
                    )),
                }
            }
            TypeExpr::Ref(name) => {
                let target = self.defs.get(name).ok_or_else(|| {
                    Error::runtime(Span::default(), format!("unknown schema '{name}'"))
                })?;
                self.check_type(value, target, path)
            }
        }
    }
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
        Span::default(),
        format!("{path}: expected {expected}, got {actual}"),
    ))
}

enum SchemaFrame {
    Unary(UnaryTypeKind),
    ResultOk,
    ResultErr(Box<TypeExpr>),
    Tuple {
        head_span: Span,
        elems: Vec<TypeExpr>,
    },
    StructField {
        name: String,
        open: bool,
        fields: Vec<(String, TypeExpr)>,
        fname: String,
    },
    VariantCase {
        name: String,
        cases: Vec<(String, Option<TypeExpr>)>,
        vname: String,
    },
}

struct SchemaParser {
    tokens: Vec<Token>,
    pos: usize,
    frames: Vec<SchemaFrame>,
}

impl SchemaParser {
    #[inline(always)]
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    #[inline(always)]
    fn peek_next(&self) -> Option<&Token> {
        self.tokens.get(self.pos + 1)
    }

    #[inline(always)]
    fn at_eof(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    #[inline(always)]
    fn advance(&mut self) {
        if !self.at_eof() {
            self.pos += 1;
        }
    }

    #[inline(always)]
    fn take_kind(&mut self) -> (TokenKind, Span) {
        let span = self.tokens[self.pos].span;
        let kind = std::mem::replace(&mut self.tokens[self.pos].kind, TokenKind::Eof);
        if kind != TokenKind::Eof {
            self.pos += 1;
        }
        (kind, span)
    }

    fn expect(&mut self, expected: TokenKind, what: &str) -> Result<(), Error> {
        let tok = self.peek();
        if tok.kind == expected {
            self.advance();
            Ok(())
        } else {
            Err(Error::parse(tok.span, format!("expected {what}")))
        }
    }

    fn expect_ident(&mut self, what: &str) -> Result<(String, Span), Error> {
        let tok = self.peek();
        if matches!(tok.kind, TokenKind::Ident(_)) {
            let (kind, span) = self.take_kind();
            let TokenKind::Ident(s) = kind else {
                unreachable!()
            };
            Ok((s, span))
        } else {
            Err(Error::parse(tok.span, format!("expected {what}")))
        }
    }

    fn peek_is_defschema(&self) -> bool {
        self.peek().kind == TokenKind::LParen
            && matches!(
                self.peek_next().map(|t| &t.kind),
                Some(TokenKind::Ident(s)) if s == "defschema"
            )
    }

    fn parse_defschema(&mut self) -> Result<(String, TypeExpr), Error> {
        self.expect(TokenKind::LParen, "'('")?;
        let (kw, _) = self.expect_ident("'defschema'")?;
        debug_assert_eq!(kw, "defschema");
        let (name, _) = self.expect_ident("schema name")?;
        let ty = self.parse_type()?;
        self.expect(TokenKind::RParen, "')'")?;
        Ok((name, ty))
    }

    fn parse_type(&mut self) -> Result<TypeExpr, Error> {
        enum Next {
            Start,
            Have(TypeExpr),
        }

        self.frames.clear();
        let mut next = Next::Start;

        'main: loop {
            match next {
                Next::Start => {
                    let (kind, span) = self.take_kind();
                    if self.frames.len() >= MAX_SYNTAX_NESTING {
                        return Err(Error::parse(
                            span,
                            format!("schema nesting limit exceeded ({MAX_SYNTAX_NESTING})"),
                        ));
                    }
                    match kind {
                        TokenKind::Ident(name) => {
                            let ty = match name.as_str() {
                                "any" => TypeExpr::Any,
                                "void" => TypeExpr::Void,
                                "i64" => TypeExpr::I64,
                                "f64" => TypeExpr::F64,
                                "bool" => TypeExpr::Bool,
                                "char" => TypeExpr::Char,
                                "string" => TypeExpr::String,
                                _ => TypeExpr::Ref(name),
                            };
                            next = Next::Have(ty);
                        }
                        TokenKind::LParen => {
                            let (head, head_span) = self.expect_ident("schema constructor")?;
                            match head.as_str() {
                                "list" => {
                                    self.frames.push(SchemaFrame::Unary(UnaryTypeKind::List));
                                    next = Next::Start;
                                }
                                "array" => {
                                    self.frames.push(SchemaFrame::Unary(UnaryTypeKind::Array));
                                    next = Next::Start;
                                }
                                "option" => {
                                    self.frames.push(SchemaFrame::Unary(UnaryTypeKind::Option));
                                    next = Next::Start;
                                }
                                "result" => {
                                    self.frames.push(SchemaFrame::ResultOk);
                                    next = Next::Start;
                                }
                                "tuple" => {
                                    if self.peek().kind == TokenKind::RParen {
                                        return Err(Error::parse(
                                            head_span,
                                            "'tuple' schema requires at least 2 element types",
                                        ));
                                    }
                                    self.frames.push(SchemaFrame::Tuple {
                                        head_span,
                                        elems: Vec::new(),
                                    });
                                    next = Next::Start;
                                }
                                "struct" | "open_struct" => {
                                    let open = head == "open_struct";
                                    let (name, _) = self.expect_ident("struct name")?;
                                    if self.peek().kind == TokenKind::RParen {
                                        self.advance();
                                        next = Next::Have(TypeExpr::Struct {
                                            name,
                                            fields: Vec::new(),
                                            open,
                                        });
                                    } else {
                                        self.expect(TokenKind::LParen, "field '('")?;
                                        let (fname, _) = self.expect_ident("field name")?;
                                        self.frames.push(SchemaFrame::StructField {
                                            name,
                                            open,
                                            fields: Vec::new(),
                                            fname,
                                        });
                                        next = Next::Start;
                                    }
                                }
                                "variant" => {
                                    let (name, _) = self.expect_ident("variant type name")?;
                                    let mut cases = Vec::new();
                                    while self.peek().kind != TokenKind::RParen {
                                        self.expect(TokenKind::LParen, "variant case '('")?;
                                        let (vname, _) = self.expect_ident("variant name")?;
                                        if self.peek().kind == TokenKind::RParen {
                                            self.advance();
                                            cases.push((vname, None));
                                        } else {
                                            self.frames.push(SchemaFrame::VariantCase {
                                                name,
                                                cases,
                                                vname,
                                            });
                                            next = Next::Start;
                                            continue 'main;
                                        }
                                    }
                                    self.advance();
                                    next = Next::Have(TypeExpr::Variant { name, cases });
                                }
                                other => {
                                    return Err(Error::parse(
                                        head_span,
                                        format!("unknown schema constructor '{other}'"),
                                    ));
                                }
                            }
                        }
                        _ => return Err(Error::parse(span, "expected type expression")),
                    }
                }
                Next::Have(ty) => {
                    let Some(frame) = self.frames.pop() else {
                        return Ok(ty);
                    };
                    match frame {
                        SchemaFrame::Unary(kind) => {
                            self.expect(TokenKind::RParen, "')'")?;
                            let wrapped = match kind {
                                UnaryTypeKind::List => TypeExpr::List(Box::new(ty)),
                                UnaryTypeKind::Array => TypeExpr::Array(Box::new(ty)),
                                UnaryTypeKind::Option => TypeExpr::Option(Box::new(ty)),
                            };
                            next = Next::Have(wrapped);
                        }
                        SchemaFrame::ResultOk => {
                            self.frames.push(SchemaFrame::ResultErr(Box::new(ty)));
                            next = Next::Start;
                        }
                        SchemaFrame::ResultErr(ok) => {
                            self.expect(TokenKind::RParen, "')'")?;
                            next = Next::Have(TypeExpr::Result(ok, Box::new(ty)));
                        }
                        SchemaFrame::Tuple {
                            head_span,
                            mut elems,
                        } => {
                            elems.push(ty);
                            if self.peek().kind == TokenKind::RParen {
                                self.advance();
                                if elems.len() < 2 {
                                    return Err(Error::parse(
                                        head_span,
                                        "'tuple' schema requires at least 2 element types",
                                    ));
                                }
                                next = Next::Have(TypeExpr::Tuple(elems));
                            } else {
                                self.frames.push(SchemaFrame::Tuple { head_span, elems });
                                next = Next::Start;
                            }
                        }
                        SchemaFrame::StructField {
                            name,
                            open,
                            mut fields,
                            fname,
                        } => {
                            self.expect(TokenKind::RParen, "')'")?;
                            fields.push((fname, ty));
                            if self.peek().kind == TokenKind::RParen {
                                self.advance();
                                next = Next::Have(TypeExpr::Struct { name, fields, open });
                            } else {
                                self.expect(TokenKind::LParen, "field '('")?;
                                let (next_fname, _) = self.expect_ident("field name")?;
                                self.frames.push(SchemaFrame::StructField {
                                    name,
                                    open,
                                    fields,
                                    fname: next_fname,
                                });
                                next = Next::Start;
                            }
                        }
                        SchemaFrame::VariantCase {
                            name,
                            mut cases,
                            vname,
                        } => {
                            self.expect(TokenKind::RParen, "')'")?;
                            cases.push((vname, Some(ty)));
                            while self.peek().kind != TokenKind::RParen {
                                self.expect(TokenKind::LParen, "variant case '('")?;
                                let (next_vname, _) = self.expect_ident("variant name")?;
                                if self.peek().kind == TokenKind::RParen {
                                    self.advance();
                                    cases.push((next_vname, None));
                                } else {
                                    self.frames.push(SchemaFrame::VariantCase {
                                        name,
                                        cases,
                                        vname: next_vname,
                                    });
                                    next = Next::Start;
                                    continue 'main;
                                }
                            }
                            self.advance();
                            next = Next::Have(TypeExpr::Variant { name, cases });
                        }
                    }
                }
            }
        }
    }
}
