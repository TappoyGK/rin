use crate::ast::{Expr, ExprKind, MatchArm, Pattern};
use crate::error::{Error, Span};
use crate::lexer::{Token, TokenKind, tokenize};

pub fn parse_program(input: &str) -> Result<Vec<Expr>, Error> {
    let tokens = tokenize(input)?;
    let mut parser = Parser { tokens, pos: 0 };
    let mut exprs = Vec::new();
    while !parser.at_eof() {
        exprs.push(parser.parse_expr()?);
    }
    Ok(exprs)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
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
            TokenKind::Ident(name) => Ok((name, tok.span)),
            _ => Err(Error::parse(tok.span, format!("expected {what}"))),
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, Error> {
        let mut expr = self.parse_primary()?;
        loop {
            let next = self.peek();
            if next.kind == TokenKind::Dot {
                self.bump();
                let (field, _) = self.expect_ident("field name after '.'")?;
                let span = expr.span;
                expr = Expr::new(
                    span,
                    ExprKind::Field {
                        target: Box::new(expr),
                        field,
                    },
                );
            } else if next.kind == TokenKind::LBracket && next.attached {
                self.bump();
                let index = self.parse_expr()?;
                self.expect(TokenKind::RBracket, "']'")?;
                let span = expr.span;
                expr = Expr::new(
                    span,
                    ExprKind::Index {
                        target: Box::new(expr),
                        index: Box::new(index),
                    },
                );
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, Error> {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::Int(n) => {
                self.bump();
                Ok(Expr::new(tok.span, ExprKind::Int(*n)))
            }
            TokenKind::Float(x) => {
                self.bump();
                Ok(Expr::new(tok.span, ExprKind::Float(*x)))
            }
            TokenKind::Str(s) => {
                self.bump();
                Ok(Expr::new(tok.span, ExprKind::Str(s.clone())))
            }
            TokenKind::Char(c) => {
                self.bump();
                Ok(Expr::new(tok.span, ExprKind::Char(*c)))
            }
            TokenKind::LBracket => self.parse_list(),
            TokenKind::LParen => self.parse_paren(),
            TokenKind::Ident(name) => self.parse_ident_expr(name.clone(), tok.span),
            _ => Err(Error::parse(
                tok.span,
                format!("unexpected token {:?}", tok.kind),
            )),
        }
    }

    fn parse_list(&mut self) -> Result<Expr, Error> {
        let start = self.expect(TokenKind::LBracket, "'['")?.span;
        let mut items = Vec::new();
        while self.peek().kind != TokenKind::RBracket {
            if self.at_eof() {
                return Err(Error::parse(start, "unterminated '['"));
            }
            items.push(self.parse_expr()?);
            if self.peek().kind == TokenKind::Comma {
                self.bump();
            }
        }
        self.bump();
        Ok(Expr::new(start, ExprKind::List(items)))
    }

    fn parse_ident_expr(&mut self, name: String, span: Span) -> Result<Expr, Error> {
        self.bump();
        match name.as_str() {
            "true" => return Ok(Expr::new(span, ExprKind::Bool(true))),
            "false" => return Ok(Expr::new(span, ExprKind::Bool(false))),
            "none" => return Ok(Expr::new(span, ExprKind::None)),
            "inf" => return Ok(Expr::new(span, ExprKind::Float(f64::INFINITY))),
            "nan" => return Ok(Expr::new(span, ExprKind::Float(f64::NAN))),
            "some" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                self.bump();
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen, "')'")?;
                return Ok(Expr::new(span, ExprKind::Some(Box::new(inner))));
            }
            "ok" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                self.bump();
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen, "')'")?;
                return Ok(Expr::new(span, ExprKind::Ok(Box::new(inner))));
            }
            "err" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                self.bump();
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen, "')'")?;
                return Ok(Expr::new(span, ExprKind::Err(Box::new(inner))));
            }
            _ => {}
        }

        let is_upper = name.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        if is_upper && self.peek().kind == TokenKind::LBrace {
            let fields = self.parse_field_map()?;
            return Ok(Expr::new(span, ExprKind::Struct { name, fields }));
        }

        if is_upper && self.peek().kind == TokenKind::Dot {
            self.bump();
            let (variant, _) = self.expect_ident("variant name after '.'")?;
            let data = if self.peek().kind == TokenKind::LParen && self.peek().attached {
                self.bump();
                let inner = self.parse_expr()?;
                self.expect(TokenKind::RParen, "')'")?;
                Some(Box::new(inner))
            } else {
                None
            };
            return Ok(Expr::new(
                span,
                ExprKind::Variant {
                    type_name: name,
                    variant,
                    data,
                },
            ));
        }

        Ok(Expr::new(span, ExprKind::Ident(name)))
    }

    fn parse_field_map(&mut self) -> Result<Vec<(String, Expr)>, Error> {
        let start = self.expect(TokenKind::LBrace, "'{'")?.span;
        let mut fields = Vec::new();
        while self.peek().kind != TokenKind::RBrace {
            if self.at_eof() {
                return Err(Error::parse(start, "unterminated '{'"));
            }
            let (key, _) = self.expect_ident("field name")?;
            self.expect(TokenKind::Colon, "':'")?;
            let val = self.parse_expr()?;
            fields.push((key, val));
            if self.peek().kind == TokenKind::Comma {
                self.bump();
            }
        }
        self.bump();
        Ok(fields)
    }

    fn parse_paren(&mut self) -> Result<Expr, Error> {
        let start = self.expect(TokenKind::LParen, "'('")?.span;
        if self.peek().kind == TokenKind::RParen {
            return Err(Error::parse(start, "empty '()' is not a valid expression"));
        }

        // Check special forms when first token is an identifier not followed by '.' or '['
        if let TokenKind::Ident(head) = &self.peek().kind {
            let is_plain_head = !matches!(
                self.peek_next().map(|t| &t.kind),
                Some(TokenKind::Dot) | Some(TokenKind::Comma)
            ) && !matches!(
                self.peek_next(),
                Some(Token {
                    kind: TokenKind::LBracket,
                    attached: true,
                    ..
                })
            );
            if is_plain_head {
                match head.as_str() {
                    "def" => return self.parse_def(start),
                    "defn" => return self.parse_defn(start),
                    "import" => return self.parse_import(start),
                    "export" => return self.parse_export(start),
                    "fn" => return self.parse_fn(start),
                    "let" => return self.parse_let(start),
                    "if" => return self.parse_if(start),
                    "when" => return self.parse_when(start),
                    "match" => return self.parse_match(start),
                    "with" => return self.parse_with(start),
                    _ => {}
                }
            }
        }

        let first = self.parse_expr()?;
        if self.peek().kind == TokenKind::Comma {
            self.bump();
            let mut items = vec![first];
            while self.peek().kind != TokenKind::RParen {
                if self.at_eof() {
                    return Err(Error::parse(start, "unterminated tuple '('"));
                }
                items.push(self.parse_expr()?);
                if self.peek().kind == TokenKind::Comma {
                    self.bump();
                } else {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "')'")?;
            return Ok(Expr::new(start, ExprKind::Tuple(items)));
        }

        let mut args = Vec::new();
        while self.peek().kind != TokenKind::RParen {
            if self.at_eof() {
                return Err(Error::parse(start, "unterminated '('"));
            }
            args.push(self.parse_expr()?);
        }
        self.bump();
        Ok(Expr::new(
            start,
            ExprKind::Call {
                callee: Box::new(first),
                args,
            },
        ))
    }

    fn parse_param_list(&mut self) -> Result<Vec<String>, Error> {
        let start = self.expect(TokenKind::LParen, "parameter list '('")?.span;
        let mut params = Vec::new();
        while self.peek().kind != TokenKind::RParen {
            if self.at_eof() {
                return Err(Error::parse(start, "unterminated parameter list"));
            }
            let (param, _) = self.expect_ident("parameter name")?;
            params.push(param);
        }
        self.bump();
        Ok(params)
    }

    fn parse_body_until_rparen(&mut self, start: Span) -> Result<Vec<Expr>, Error> {
        let mut body = Vec::new();
        while self.peek().kind != TokenKind::RParen {
            if self.at_eof() {
                return Err(Error::parse(start, "unterminated '('"));
            }
            body.push(self.parse_expr()?);
        }
        self.bump();
        if body.is_empty() {
            return Err(Error::parse(start, "body cannot be empty"));
        }
        Ok(body)
    }

    fn parse_def(&mut self, start: Span) -> Result<Expr, Error> {
        self.bump(); // def
        let (name, _) = self.expect_ident("variable name")?;
        let value = self.parse_expr()?;
        self.expect(TokenKind::RParen, "')'")?;
        Ok(Expr::new(
            start,
            ExprKind::Def {
                name,
                value: Box::new(value),
            },
        ))
    }

    fn parse_import(&mut self, start: Span) -> Result<Expr, Error> {
        self.bump(); // import
        let (name, _) = self.expect_ident("module name")?;
        self.expect(TokenKind::RParen, "')'")?;
        Ok(Expr::new(start, ExprKind::Import { name }))
    }

    fn parse_export(&mut self, start: Span) -> Result<Expr, Error> {
        self.bump(); // export
        let mut names = Vec::new();
        while self.peek().kind != TokenKind::RParen {
            if self.at_eof() {
                return Err(Error::parse(start, "unterminated export '('"));
            }
            let (name, _) = self.expect_ident("exported symbol name")?;
            names.push(name);
        }
        self.bump();
        Ok(Expr::new(start, ExprKind::Export { names }))
    }

    fn parse_defn(&mut self, start: Span) -> Result<Expr, Error> {
        self.bump(); // defn
        let (name, _) = self.expect_ident("function name")?;
        let params = self.parse_param_list()?;
        let body = self.parse_body_until_rparen(start)?;
        Ok(Expr::new(start, ExprKind::Defn { name, params, body }))
    }

    fn parse_fn(&mut self, start: Span) -> Result<Expr, Error> {
        self.bump(); // fn
        let params = self.parse_param_list()?;
        let body = self.parse_body_until_rparen(start)?;
        Ok(Expr::new(start, ExprKind::Fn { params, body }))
    }

    fn parse_let(&mut self, start: Span) -> Result<Expr, Error> {
        self.bump(); // let
        let bind_start = self.expect(TokenKind::LParen, "let bindings '('")?.span;
        let mut bindings = Vec::new();
        while self.peek().kind != TokenKind::RParen {
            if self.at_eof() {
                return Err(Error::parse(bind_start, "unterminated let bindings"));
            }
            let (name, _) = self.expect_ident("binding name")?;
            let val = self.parse_expr()?;
            bindings.push((name, val));
        }
        self.bump();
        let body = self.parse_body_until_rparen(start)?;
        Ok(Expr::new(start, ExprKind::Let { bindings, body }))
    }

    fn parse_if(&mut self, start: Span) -> Result<Expr, Error> {
        self.bump(); // if
        let cond = self.parse_expr()?;
        let then_branch = self.parse_expr()?;
        if self.peek().kind == TokenKind::RParen {
            return Err(Error::parse(
                start,
                "'if' requires 3 arguments (cond, then, else); use 'when' when no value is needed",
            ));
        }
        let else_branch = self.parse_expr()?;
        self.expect(TokenKind::RParen, "')'")?;
        Ok(Expr::new(
            start,
            ExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch: Box::new(else_branch),
            },
        ))
    }

    fn parse_when(&mut self, start: Span) -> Result<Expr, Error> {
        self.bump(); // when
        let cond = self.parse_expr()?;
        let body = self.parse_body_until_rparen(start)?;
        Ok(Expr::new(
            start,
            ExprKind::When {
                cond: Box::new(cond),
                body,
            },
        ))
    }

    fn parse_with(&mut self, start: Span) -> Result<Expr, Error> {
        self.bump(); // with
        let target = self.parse_expr()?;
        let fields = self.parse_field_map()?;
        self.expect(TokenKind::RParen, "')'")?;
        Ok(Expr::new(
            start,
            ExprKind::With {
                target: Box::new(target),
                fields,
            },
        ))
    }

    fn parse_match(&mut self, start: Span) -> Result<Expr, Error> {
        self.bump(); // match
        let scrutinee = self.parse_expr()?;
        let mut arms = Vec::new();
        while self.peek().kind != TokenKind::RParen {
            if self.at_eof() {
                return Err(Error::parse(start, "unterminated match"));
            }
            let arm_start = self.expect(TokenKind::LParen, "match arm '('")?.span;
            let pattern = self.parse_pattern()?;
            let body = self.parse_body_until_rparen(arm_start)?;
            arms.push(MatchArm {
                span: arm_start,
                pattern,
                body,
            });
        }
        self.bump();
        Ok(Expr::new(
            start,
            ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
        ))
    }

    fn parse_pattern(&mut self) -> Result<Pattern, Error> {
        let tok = self.bump();
        match tok.kind {
            TokenKind::Int(n) => Ok(Pattern::Int(n)),
            TokenKind::Str(s) => Ok(Pattern::Str(s)),
            TokenKind::Char(c) => Ok(Pattern::Char(c)),
            TokenKind::Ident(name) => match name.as_str() {
                "_" => Ok(Pattern::Wildcard),
                "true" => Ok(Pattern::Bool(true)),
                "false" => Ok(Pattern::Bool(false)),
                "none" => Ok(Pattern::None),
                "some" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::Some(self.parse_opt_binding()?))
                }
                "ok" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::Ok(self.parse_opt_binding()?))
                }
                "err" if self.peek().kind == TokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::Err(self.parse_opt_binding()?))
                }
                _ if name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) => {
                    self.expect(TokenKind::Dot, "'.' in variant pattern")?;
                    let (variant, _) = self.expect_ident("variant name")?;
                    let binding = if self.peek().kind == TokenKind::LParen && self.peek().attached {
                        self.parse_opt_binding()?
                    } else {
                        None
                    };
                    Ok(Pattern::Variant {
                        type_name: name,
                        variant,
                        binding,
                    })
                }
                _ => Ok(Pattern::Var(name)),
            },
            _ => Err(Error::parse(tok.span, "invalid match pattern")),
        }
    }

    fn parse_opt_binding(&mut self) -> Result<Option<String>, Error> {
        self.expect(TokenKind::LParen, "'('")?;
        let (name, _) = self.expect_ident("binding name or '_'")?;
        self.expect(TokenKind::RParen, "')'")?;
        Ok(if name == "_" { None } else { Some(name) })
    }
}
