//! Rin の値構造を検証するスキーマ DSL の構文解析と照合を実装する。

mod check;
use std::collections::BTreeMap;
use std::fmt;

use crate::error::{Error, Span};
use crate::lexer::{Token, TokenKind, tokenize};
use crate::parser::MAX_SYNTAX_NESTING;
use crate::value::Value;

/// スキーマの型式 AST。
#[derive(Debug, Clone)]
pub(crate) enum TypeExpr {
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

#[cold]
#[inline(never)]
fn drop_deep_type(ty: &mut TypeExpr) {
    if !has_type_children(ty) {
        return;
    }
    let mut next = None;
    let mut work = Vec::new();
    take_type_children(ty, &mut next, &mut work);
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

/// `Value` の構造を検証するコンパイル済み Rin スキーマ。
///
/// 深い入れ子（深さ 32 以上）を含むスキーマでは、`Clone`・`Drop`・`Debug` を反復処理へ切り替えるため、
/// `{:#?}` 出力は 1 行形式となる。
pub struct Schema {
    defs: BTreeMap<String, TypeExpr>,
    root: TypeExpr,
    has_deep: bool,
}

impl Clone for Schema {
    #[inline]
    fn clone(&self) -> Self {
        if !self.has_deep {
            Self {
                defs: self.defs.clone(),
                root: self.root.clone(),
                has_deep: false,
            }
        } else {
            self.clone_deep()
        }
    }
}

impl fmt::Debug for Schema {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.has_deep {
            f.debug_struct("Schema")
                .field("defs", &self.defs)
                .field("root", &self.root)
                .finish()
        } else {
            self.fmt_deep(f)
        }
    }
}

impl Drop for Schema {
    #[inline]
    fn drop(&mut self) {
        if self.has_deep {
            self.drop_deep();
        }
    }
}

impl Schema {
    #[cold]
    #[inline(never)]
    fn clone_deep(&self) -> Self {
        let defs = self
            .defs
            .iter()
            .map(|(k, v)| (k.clone(), v.clone_iter()))
            .collect();
        let root = self.root.clone_iter();
        Self {
            defs,
            root,
            has_deep: true,
        }
    }

    #[cold]
    #[inline(never)]
    fn fmt_deep(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct DeepTy<'a>(&'a TypeExpr);
        impl fmt::Debug for DeepTy<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt_iter(f)
            }
        }
        struct DeepMap<'a>(&'a BTreeMap<String, TypeExpr>);
        impl fmt::Debug for DeepMap<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_map()
                    .entries(self.0.iter().map(|(k, v)| (k, DeepTy(v))))
                    .finish()
            }
        }
        f.debug_struct("Schema")
            .field("defs", &DeepMap(&self.defs))
            .field("root", &DeepTy(&self.root))
            .finish()
    }

    #[cold]
    #[inline(never)]
    fn drop_deep(&mut self) {
        drop_deep_type(&mut self.root);
        for (_, mut ty) in std::mem::take(&mut self.defs) {
            drop_deep_type(&mut ty);
        }
    }

    /// 0 個以上の `(defschema Name TypeExpr)` 宣言と末尾のルート `TypeExpr` を解析する。
    pub fn parse(source: &str) -> Result<Self, Error> {
        let tokens = tokenize(source)?;
        let mut p = SchemaParser {
            tokens,
            pos: 0,
            frames: Vec::new(),
            defs: BTreeMap::new(),
            max_depth: 0,
        };
        let mut root = None;

        while !p.at_eof() {
            if p.peek_is_defschema() {
                let (name, ty) = p.parse_defschema()?;
                p.defs.insert(name, ty);
            } else {
                let ty = p.parse_type()?;
                if !p.at_eof() {
                    if p.max_depth >= SHALLOW_TYPE_DEPTH {
                        let mut ty = ty;
                        drop_deep_type(&mut ty);
                    }
                    return Err(Error::parse(
                        "schema.program",
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
                "schema.program",
                Span::new(1, 1),
                "schema must end with a root type expression",
            )
        })?;
        let defs = std::mem::take(&mut p.defs);
        let has_deep = p.max_depth >= SHALLOW_TYPE_DEPTH;
        let schema = Self {
            defs,
            root,
            has_deep,
        };
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
                    "schema.reference",
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
                            "schema.reference",
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
        check::check(self, value)
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
        "schema.type",
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
    defs: BTreeMap<String, TypeExpr>,
    max_depth: usize,
}

impl Drop for SchemaParser {
    fn drop(&mut self) {
        if self.max_depth < SHALLOW_TYPE_DEPTH {
            return;
        }
        for frame in std::mem::take(&mut self.frames) {
            match frame {
                SchemaFrame::Unary(_) | SchemaFrame::ResultOk => {}
                SchemaFrame::ResultErr(mut ty) => drop_deep_type(&mut ty),
                SchemaFrame::Tuple { elems, .. } => {
                    for mut t in elems {
                        drop_deep_type(&mut t);
                    }
                }
                SchemaFrame::StructField { fields, .. } => {
                    for (_, mut t) in fields {
                        drop_deep_type(&mut t);
                    }
                }
                SchemaFrame::VariantCase { cases, .. } => {
                    for (_, opt) in cases {
                        if let Some(mut t) = opt {
                            drop_deep_type(&mut t);
                        }
                    }
                }
            }
        }
        for (_, mut ty) in std::mem::take(&mut self.defs) {
            drop_deep_type(&mut ty);
        }
    }
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
            Err(Error::parse(
                "schema.syntax",
                tok.span,
                format!("expected {what}"),
            ))
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
            Err(Error::parse(
                "schema.syntax",
                tok.span,
                format!("expected {what}"),
            ))
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
                    let depth = self.frames.len() + 1;
                    if depth > self.max_depth {
                        self.max_depth = depth;
                    }
                    if self.frames.len() >= MAX_SYNTAX_NESTING {
                        return Err(Error::parse(
                            "src.nesting-limit",
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
                                            "schema.tuple",
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
                                        "schema.syntax",
                                        head_span,
                                        format!("unknown schema constructor '{other}'"),
                                    ));
                                }
                            }
                        }
                        _ => {
                            return Err(Error::parse(
                                "schema.syntax",
                                span,
                                "expected type expression",
                            ));
                        }
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
                                        "schema.tuple",
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
