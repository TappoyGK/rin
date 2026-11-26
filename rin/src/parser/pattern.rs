use super::*;

impl Parser<'_, '_, '_> {
    pub(super) fn parse_param_list(&mut self) -> Result<RawVec<RawStr>, ErrRecorded> {
        let start = self.expect(WorkTokenKind::LParen, "parameter list '('")?;
        let mut params = RawVec::new();
        while self.peek().kind != WorkTokenKind::RParen {
            if self.at_eof() {
                return Err(self.parse_err("src.syntax", start, "unterminated parameter list"));
            }
            let (param, pspan) = self.expect_ident("parameter name")?;
            if params.push_work(self.work, self.core, param).is_none() {
                return Err(self.oom(pspan));
            }
        }
        self.advance()?;
        Ok(params)
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
                        type_name: name,
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
