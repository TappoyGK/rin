use super::*;

impl Parser<'_> {
    pub(super) fn parse_param_list(&mut self) -> Result<Vec<String>, Error> {
        let start = self.expect(TokenKind::LParen, "parameter list '('")?;
        let mut params = Vec::new();
        while self.peek().kind != TokenKind::RParen {
            if self.at_eof() {
                return Err(Error::parse(
                    "src.syntax",
                    start,
                    "unterminated parameter list",
                ));
            }
            let (param, _) = self.expect_ident("parameter name")?;
            params.push(param);
        }
        self.advance();
        Ok(params)
    }

    pub(super) fn parse_pattern(&mut self) -> Result<Pattern, Error> {
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
            _ => Err(Error::parse("pat.kinds", tok.span, "invalid match pattern")),
        }
    }

    fn parse_opt_binding(&mut self) -> Result<Option<String>, Error> {
        self.expect(TokenKind::LParen, "'('")?;
        let (name, _) = self.expect_ident("binding name or '_'")?;
        self.expect(TokenKind::RParen, "')'")?;
        Ok(if name == "_" { None } else { Some(name) })
    }
}
