//! スキーマDSLの解析と参照先の検証。
use super::{
    Schema,
    drop::drop_deep_type,
    types::{SHALLOW_TYPE_DEPTH, TypeExpr, UnaryTypeKind, has_type_children},
};
use crate::{
    error::{Error, Span},
    lexer::{Token, TokenKind, tokenize},
    parser::MAX_SYNTAX_NESTING,
};
use std::collections::BTreeMap;

impl Schema {
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
                | TypeExpr::Text
                | TypeExpr::Ident => {}
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
                                "text" => TypeExpr::Text,
                                "ident" => TypeExpr::Ident,
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
