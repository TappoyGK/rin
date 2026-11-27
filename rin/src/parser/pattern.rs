use super::*;

impl Parser<'_, '_, '_> {
    pub(super) fn parse_param_list(&mut self) -> Result<(RawVec<Param>, usize), ErrRecorded> {
        let start = self.expect(WorkTokenKind::LParen, "parameter list '('")?;
        let base_depth = self.frames.len() + 1;
        let mut params = RawVec::new();
        let mut max_ty_depth = 0usize;
        while self.peek().kind != WorkTokenKind::RParen {
            if self.at_eof() {
                return Err(self.parse_err("src.syntax", start, "unterminated parameter list"));
            }
            let param = if self.peek().kind == WorkTokenKind::LParen {
                self.advance()?;
                let (name, _) = self.expect_ident("parameter name")?;
                let (ty, ty_depth) = self.parse_type_expr(base_depth)?;
                self.expect(WorkTokenKind::RParen, "')' after parameter type")?;
                max_ty_depth = max_ty_depth.max(ty_depth);
                let boxed = self.alloc_box(ty, ty.span)?;
                Param {
                    name,
                    ty: Some(boxed),
                }
            } else {
                let (name, _) = self.expect_ident("parameter name")?;
                Param { name, ty: None }
            };
            if params.push_work(self.work, self.core, param).is_none() {
                return Err(self.oom(start));
            }
        }
        self.advance()?;
        Ok((params, max_ty_depth))
    }

    pub(super) fn parse_opt_return_type(
        &mut self,
    ) -> Result<(Option<WorkBox<TypeExpr>>, usize), ErrRecorded> {
        if let WorkTokenKind::Ident(s) = self.peek().kind
            && s.as_str() == "->"
        {
            self.advance()?;
            let (ty, depth) = self.parse_type_expr(self.frames.len() + 1)?;
            let boxed = self.alloc_box(ty, ty.span)?;
            return Ok((Some(boxed), depth));
        }
        Ok((None, 0))
    }

    pub(super) fn parse_pattern(&mut self) -> Result<Pattern, ErrRecorded> {
        let tok = self.bump()?;
        match tok.kind {
            WorkTokenKind::Int(n) => Ok(Pattern::Int(n)),
            WorkTokenKind::Str(s) => Ok(Pattern::Str(s)),
            WorkTokenKind::Char(c) => Ok(Pattern::Char(c)),
            WorkTokenKind::Quote => {
                let (name, _) = self.expect_ident("identifier after ''' in pattern")?;
                Ok(Pattern::Ident(name))
            }
            WorkTokenKind::Ident(name) => match name.as_str() {
                "_" => Ok(Pattern::Wildcard),
                "true" => Ok(Pattern::Bool(true)),
                "false" => Ok(Pattern::Bool(false)),
                "none" => Ok(Pattern::None),
                "some" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::Some(self.parse_opt_binding()?))
                }
                "ok" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::Ok(self.parse_opt_binding()?))
                }
                "err" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::Err(self.parse_opt_binding()?))
                }
                "i64" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::DatumType {
                        kind: DatumTypePat::I64,
                        binding: self.parse_opt_binding()?,
                    })
                }
                "f64" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::DatumType {
                        kind: DatumTypePat::F64,
                        binding: self.parse_opt_binding()?,
                    })
                }
                "bool" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::DatumType {
                        kind: DatumTypePat::Bool,
                        binding: self.parse_opt_binding()?,
                    })
                }
                "char" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::DatumType {
                        kind: DatumTypePat::Char,
                        binding: self.parse_opt_binding()?,
                    })
                }
                "text" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::DatumType {
                        kind: DatumTypePat::Text,
                        binding: self.parse_opt_binding()?,
                    })
                }
                "ident" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::DatumType {
                        kind: DatumTypePat::Ident,
                        binding: self.parse_opt_binding()?,
                    })
                }
                "list" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                    Ok(Pattern::DatumType {
                        kind: DatumTypePat::List,
                        binding: self.parse_opt_binding()?,
                    })
                }
                s if s.chars().next().is_some_and(|c| c.is_ascii_uppercase()) => {
                    self.expect(WorkTokenKind::Dot, "'.' in variant pattern")?;
                    let (variant, _) = self.expect_ident("variant name")?;
                    let binding =
                        if self.peek().kind == WorkTokenKind::LParen && self.peek().attached {
                            self.parse_opt_binding()?
                        } else {
                            None
                        };
                    Ok(Pattern::Variant {
                        module: None,
                        type_name: name,
                        variant,
                        binding,
                    })
                }
                _ if self.peek().kind == WorkTokenKind::Dot => {
                    self.advance()?;
                    let (type_name, tspan) = self.expect_ident("variant type name after '.'")?;
                    if !type_name
                        .as_str()
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_uppercase())
                    {
                        return Err(self.parse_err(
                            "pat.kinds",
                            tspan,
                            "variant type name must start with an uppercase letter",
                        ));
                    }
                    self.expect(WorkTokenKind::Dot, "'.' in variant pattern")?;
                    let (variant, _) = self.expect_ident("variant name")?;
                    let binding =
                        if self.peek().kind == WorkTokenKind::LParen && self.peek().attached {
                            self.parse_opt_binding()?
                        } else {
                            None
                        };
                    Ok(Pattern::Variant {
                        module: Some(name),
                        type_name,
                        variant,
                        binding,
                    })
                }
                _ => Ok(Pattern::Var(name)),
            },
            _ => Err(self.parse_err("pat.kinds", tok.span, "invalid match pattern")),
        }
    }

    fn parse_opt_binding(&mut self) -> Result<Option<RawStr>, ErrRecorded> {
        self.expect(WorkTokenKind::LParen, "'('")?;
        let (name, _) = self.expect_ident("binding name or '_'")?;
        self.expect(WorkTokenKind::RParen, "')'")?;
        Ok(if name.as_str() == "_" {
            None
        } else {
            Some(name)
        })
    }
}
