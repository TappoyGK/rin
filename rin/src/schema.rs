use std::collections::BTreeMap;

use crate::error::{Error, Span};
use crate::lexer::{Token, TokenKind, tokenize};
use crate::value::Value;

#[derive(Debug, Clone)]
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

/// Compiled Rin schema for validating `Value` structures.
#[derive(Debug, Clone)]
pub struct Schema {
    defs: BTreeMap<String, TypeExpr>,
    root: TypeExpr,
}

impl Schema {
    /// Parses one or more `(defschema Name TypeExpr)` declarations followed by a root `TypeExpr`.
    pub fn parse(source: &str) -> Result<Self, Error> {
        let tokens = tokenize(source)?;
        let mut p = SchemaParser { tokens, pos: 0 };
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
            Error::parse(Span::new(1, 1), "schema must end with a root type expression")
        })?;
        let schema = Self { defs, root };
        schema.validate_refs(&schema.root)?;
        for ty in schema.defs.values() {
            schema.validate_refs(ty)?;
        }
        Ok(schema)
    }

    fn validate_refs(&self, ty: &TypeExpr) -> Result<(), Error> {
        match ty {
            TypeExpr::Any
            | TypeExpr::Void
            | TypeExpr::I64
            | TypeExpr::F64
            | TypeExpr::Bool
            | TypeExpr::Char
            | TypeExpr::String => Ok(()),
            TypeExpr::List(inner) | TypeExpr::Array(inner) | TypeExpr::Option(inner) => {
                self.validate_refs(inner)
            }
            TypeExpr::Result(ok, err) => {
                self.validate_refs(ok)?;
                self.validate_refs(err)
            }
            TypeExpr::Tuple(items) => {
                for item in items {
                    self.validate_refs(item)?;
                }
                Ok(())
            }
            TypeExpr::Struct { fields, .. } => {
                for (_, fty) in fields {
                    self.validate_refs(fty)?;
                }
                Ok(())
            }
            TypeExpr::Variant { cases, .. } => {
                for (_, payload) in cases {
                    if let Some(pty) = payload {
                        self.validate_refs(pty)?;
                    }
                }
                Ok(())
            }
            TypeExpr::Ref(name) => {
                if self.defs.contains_key(name) {
                    Ok(())
                } else {
                    Err(Error::parse(
                        Span::new(1, 1),
                        format!("unknown schema name '{name}'"),
                    ))
                }
            }
        }
    }

    /// Checks whether `value` conforms to this schema.
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

struct SchemaParser {
    tokens: Vec<Token>,
    pos: usize,
}

impl SchemaParser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_next(&self) -> Option<&Token> {
        self.tokens.get(self.pos + 1)
    }

    fn at_eof(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    fn bump(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if !self.at_eof() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, expected: TokenKind, what: &str) -> Result<Token, Error> {
        let tok = self.peek().clone();
        if tok.kind == expected {
            self.pos += 1;
            Ok(tok)
        } else {
            Err(Error::parse(tok.span, format!("expected {what}")))
        }
    }

    fn expect_ident(&mut self, what: &str) -> Result<(String, Span), Error> {
        let tok = self.bump();
        match tok.kind {
            TokenKind::Ident(s) => Ok((s, tok.span)),
            _ => Err(Error::parse(tok.span, format!("expected {what}"))),
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
        let tok = self.bump();
        match tok.kind {
            TokenKind::Ident(name) => match name.as_str() {
                "any" => Ok(TypeExpr::Any),
                "void" => Ok(TypeExpr::Void),
                "i64" => Ok(TypeExpr::I64),
                "f64" => Ok(TypeExpr::F64),
                "bool" => Ok(TypeExpr::Bool),
                "char" => Ok(TypeExpr::Char),
                "string" => Ok(TypeExpr::String),
                _ => Ok(TypeExpr::Ref(name)),
            },
            TokenKind::LParen => {
                let (head, head_span) = self.expect_ident("schema constructor")?;
                let ty = match head.as_str() {
                    "list" => TypeExpr::List(Box::new(self.parse_type()?)),
                    "array" => TypeExpr::Array(Box::new(self.parse_type()?)),
                    "option" => TypeExpr::Option(Box::new(self.parse_type()?)),
                    "result" => {
                        let ok = self.parse_type()?;
                        let err = self.parse_type()?;
                        TypeExpr::Result(Box::new(ok), Box::new(err))
                    }
                    "tuple" => {
                        let mut elems = Vec::new();
                        while self.peek().kind != TokenKind::RParen {
                            elems.push(self.parse_type()?);
                        }
                        if elems.len() < 2 {
                            return Err(Error::parse(
                                head_span,
                                "'tuple' schema requires at least 2 element types",
                            ));
                        }
                        TypeExpr::Tuple(elems)
                    }
                    "struct" | "open_struct" => {
                        let open = head == "open_struct";
                        let (name, _) = self.expect_ident("struct name")?;
                        let mut fields = Vec::new();
                        while self.peek().kind != TokenKind::RParen {
                            self.expect(TokenKind::LParen, "field '('")?;
                            let (fname, _) = self.expect_ident("field name")?;
                            let fty = self.parse_type()?;
                            self.expect(TokenKind::RParen, "')'")?;
                            fields.push((fname, fty));
                        }
                        TypeExpr::Struct { name, fields, open }
                    }
                    "variant" => {
                        let (name, _) = self.expect_ident("variant type name")?;
                        let mut cases = Vec::new();
                        while self.peek().kind != TokenKind::RParen {
                            self.expect(TokenKind::LParen, "variant case '('")?;
                            let (vname, _) = self.expect_ident("variant name")?;
                            let payload = if self.peek().kind == TokenKind::RParen {
                                None
                            } else {
                                Some(self.parse_type()?)
                            };
                            self.expect(TokenKind::RParen, "')'")?;
                            cases.push((vname, payload));
                        }
                        TypeExpr::Variant { name, cases }
                    }
                    other => {
                        return Err(Error::parse(
                            head_span,
                            format!("unknown schema constructor '{other}'"),
                        ));
                    }
                };
                self.expect(TokenKind::RParen, "')'")?;
                Ok(ty)
            }
            _ => Err(Error::parse(tok.span, "expected type expression")),
        }
    }
}
