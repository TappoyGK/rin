//! `quote` / `quasiquote` 内のデータリテラル（Datum）の反復構文解析。
use super::*;

impl Parser<'_, '_, '_> {
    pub(super) fn start_datum(
        &mut self,
        qq_depth: usize,
        allow_splice: bool,
    ) -> Result<NextStep, ErrRecorded> {
        let tok = self.peek();
        let span = tok.span;
        match tok.kind {
            WorkTokenKind::Int(n) => {
                self.advance()?;
                Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::Int(n)),
                    1,
                    qq_depth,
                ))
            }
            WorkTokenKind::Float(x) => {
                self.advance()?;
                Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::Float(x)),
                    1,
                    qq_depth,
                ))
            }
            WorkTokenKind::Str(s) => {
                self.advance()?;
                Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::Str(s)),
                    1,
                    qq_depth,
                ))
            }
            WorkTokenKind::Char(c) => {
                self.advance()?;
                Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::Char(c)),
                    1,
                    qq_depth,
                ))
            }
            WorkTokenKind::Quote => {
                self.advance()?;
                self.push_frame(
                    ParseFrame::DatumWrap {
                        span,
                        tag: "quote",
                        is_paren: false,
                        outer_qq_depth: qq_depth,
                    },
                    span,
                )?;
                Ok(NextStep::StartDatum {
                    qq_depth,
                    allow_splice: false,
                })
            }
            WorkTokenKind::Quasiquote => {
                self.advance()?;
                self.push_frame(
                    ParseFrame::DatumWrap {
                        span,
                        tag: "quasiquote",
                        is_paren: false,
                        outer_qq_depth: qq_depth,
                    },
                    span,
                )?;
                let next_depth = if qq_depth > 0 { qq_depth + 1 } else { 0 };
                Ok(NextStep::StartDatum {
                    qq_depth: next_depth,
                    allow_splice: false,
                })
            }
            WorkTokenKind::Unquote => {
                self.advance()?;
                if qq_depth == 1 {
                    self.push_frame(
                        ParseFrame::DatumUnquote {
                            span,
                            is_paren: false,
                            is_splicing: false,
                        },
                        span,
                    )?;
                    Ok(NextStep::StartExpr)
                } else {
                    self.push_frame(
                        ParseFrame::DatumWrap {
                            span,
                            tag: "unquote",
                            is_paren: false,
                            outer_qq_depth: qq_depth,
                        },
                        span,
                    )?;
                    Ok(NextStep::StartDatum {
                        qq_depth: qq_depth.saturating_sub(1),
                        allow_splice: false,
                    })
                }
            }
            WorkTokenKind::UnquoteSplicing => {
                self.advance()?;
                if qq_depth == 1 {
                    if !allow_splice {
                        return Err(self.parse_err(
                            "src.syntax",
                            span,
                            "unquote-splicing ',@' is only allowed inside a list",
                        ));
                    }
                    self.push_frame(
                        ParseFrame::DatumUnquote {
                            span,
                            is_paren: false,
                            is_splicing: true,
                        },
                        span,
                    )?;
                    Ok(NextStep::StartExpr)
                } else {
                    self.push_frame(
                        ParseFrame::DatumWrap {
                            span,
                            tag: "unquote_splicing",
                            is_paren: false,
                            outer_qq_depth: qq_depth,
                        },
                        span,
                    )?;
                    Ok(NextStep::StartDatum {
                        qq_depth: qq_depth.saturating_sub(1),
                        allow_splice: false,
                    })
                }
            }
            WorkTokenKind::LBracket => {
                self.advance()?;
                let list_kw = self.intern_work_str("list", span)?;
                let mut items = RawVec::new();
                if items
                    .push_work(
                        self.work,
                        self.core,
                        Datum::new(span, DatumKind::Ident(list_kw)),
                    )
                    .is_none()
                {
                    return Err(self.oom(span));
                }
                if self.peek().kind == WorkTokenKind::RBracket {
                    self.advance()?;
                    return Ok(NextStep::HaveDatum(
                        Datum::new(span, DatumKind::List(items)),
                        1,
                        qq_depth,
                    ));
                }
                let dl = self.alloc_box(
                    DatumListFrame {
                        start: span,
                        items,
                        max_child: 0,
                        qq_depth,
                        close: WorkTokenKind::RBracket,
                    },
                    span,
                )?;
                self.push_frame(ParseFrame::DatumList(dl), span)?;
                Ok(NextStep::StartDatum {
                    qq_depth,
                    allow_splice: true,
                })
            }
            WorkTokenKind::LParen => self.start_datum_paren(qq_depth, allow_splice),
            WorkTokenKind::Ident(name) => self.start_datum_ident(name, span, qq_depth),
            other => Err(self.parse_err_fmt(
                "src.syntax",
                span,
                format_args!("unexpected token {other:?}"),
            )),
        }
    }

    fn start_datum_paren(
        &mut self,
        qq_depth: usize,
        allow_splice: bool,
    ) -> Result<NextStep, ErrRecorded> {
        let start = self.expect(WorkTokenKind::LParen, "'('")?;
        if self.peek().kind == WorkTokenKind::RParen {
            self.advance()?;
            return Ok(NextStep::HaveDatum(
                Datum::new(start, DatumKind::List(RawVec::new())),
                1,
                qq_depth,
            ));
        }

        if qq_depth >= 1 {
            let special = if let WorkTokenKind::Ident(head) = self.peek().kind {
                let next_tok = self.peek_next()?;
                let is_plain_head = !matches!(next_tok.map(|t| t.kind), Some(WorkTokenKind::Dot))
                    && !matches!(
                        next_tok,
                        Some(WorkToken {
                            kind: WorkTokenKind::LBracket,
                            attached: true,
                            ..
                        })
                    );
                if is_plain_head { Some(head) } else { None }
            } else {
                None
            };
            if let Some(head) = special {
                match head.as_str() {
                    "quasiquote" => {
                        self.advance()?;
                        self.push_frame(
                            ParseFrame::DatumWrap {
                                span: start,
                                tag: "quasiquote",
                                is_paren: true,
                                outer_qq_depth: qq_depth,
                            },
                            start,
                        )?;
                        return Ok(NextStep::StartDatum {
                            qq_depth: qq_depth + 1,
                            allow_splice: false,
                        });
                    }
                    "unquote" => {
                        self.advance()?;
                        if qq_depth == 1 {
                            self.push_frame(
                                ParseFrame::DatumUnquote {
                                    span: start,
                                    is_paren: true,
                                    is_splicing: false,
                                },
                                start,
                            )?;
                            return Ok(NextStep::StartExpr);
                        } else {
                            self.push_frame(
                                ParseFrame::DatumWrap {
                                    span: start,
                                    tag: "unquote",
                                    is_paren: true,
                                    outer_qq_depth: qq_depth,
                                },
                                start,
                            )?;
                            return Ok(NextStep::StartDatum {
                                qq_depth: qq_depth - 1,
                                allow_splice: false,
                            });
                        }
                    }
                    "unquote_splicing" => {
                        self.advance()?;
                        if qq_depth == 1 {
                            if !allow_splice {
                                return Err(self.parse_err(
                                    "src.syntax",
                                    start,
                                    "unquote-splicing ',@' is only allowed inside a list",
                                ));
                            }
                            self.push_frame(
                                ParseFrame::DatumUnquote {
                                    span: start,
                                    is_paren: true,
                                    is_splicing: true,
                                },
                                start,
                            )?;
                            return Ok(NextStep::StartExpr);
                        } else {
                            self.push_frame(
                                ParseFrame::DatumWrap {
                                    span: start,
                                    tag: "unquote_splicing",
                                    is_paren: true,
                                    outer_qq_depth: qq_depth,
                                },
                                start,
                            )?;
                            return Ok(NextStep::StartDatum {
                                qq_depth: qq_depth - 1,
                                allow_splice: false,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        let dl = self.alloc_box(
            DatumListFrame {
                start,
                items: RawVec::new(),
                max_child: 0,
                qq_depth,
                close: WorkTokenKind::RParen,
            },
            start,
        )?;
        self.push_frame(ParseFrame::DatumList(dl), start)?;
        Ok(NextStep::StartDatum {
            qq_depth,
            allow_splice: true,
        })
    }

    fn start_datum_ident(
        &mut self,
        name: RawStr,
        span: Span,
        qq_depth: usize,
    ) -> Result<NextStep, ErrRecorded> {
        self.advance()?;
        match name.as_str() {
            "true" => {
                return Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::Bool(true)),
                    1,
                    qq_depth,
                ));
            }
            "false" => {
                return Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::Bool(false)),
                    1,
                    qq_depth,
                ));
            }
            "none" => {
                return Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::None),
                    1,
                    qq_depth,
                ));
            }
            "inf" => {
                return Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::Float(f64::INFINITY)),
                    1,
                    qq_depth,
                ));
            }
            "nan" => {
                return Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::Float(f64::NAN)),
                    1,
                    qq_depth,
                ));
            }
            "some" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                self.advance()?;
                self.push_frame(
                    ParseFrame::DatumWrap {
                        span,
                        tag: "some",
                        is_paren: true,
                        outer_qq_depth: qq_depth,
                    },
                    span,
                )?;
                return Ok(NextStep::StartDatum {
                    qq_depth,
                    allow_splice: false,
                });
            }
            "ok" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                self.advance()?;
                self.push_frame(
                    ParseFrame::DatumWrap {
                        span,
                        tag: "ok",
                        is_paren: true,
                        outer_qq_depth: qq_depth,
                    },
                    span,
                )?;
                return Ok(NextStep::StartDatum {
                    qq_depth,
                    allow_splice: false,
                });
            }
            "err" if self.peek().kind == WorkTokenKind::LParen && self.peek().attached => {
                self.advance()?;
                self.push_frame(
                    ParseFrame::DatumWrap {
                        span,
                        tag: "err",
                        is_paren: true,
                        outer_qq_depth: qq_depth,
                    },
                    span,
                )?;
                return Ok(NextStep::StartDatum {
                    qq_depth,
                    allow_splice: false,
                });
            }
            _ => {}
        }

        let is_upper = name
            .as_str()
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase());
        if is_upper && self.peek().kind == WorkTokenKind::LBrace {
            let brace_start = self.peek().span;
            self.advance()?;
            let struct_kw = self.intern_work_str("struct", span)?;
            let mut items = RawVec::new();
            for item in [
                Datum::new(span, DatumKind::Ident(struct_kw)),
                Datum::new(span, DatumKind::Ident(name)),
            ] {
                if items.push_work(self.work, self.core, item).is_none() {
                    return Err(self.oom(span));
                }
            }
            if self.peek().kind == WorkTokenKind::RBrace {
                self.advance()?;
                return Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::List(items)),
                    1,
                    qq_depth,
                ));
            }
            let (key, key_span) = self.expect_ident("field name")?;
            self.expect(WorkTokenKind::Colon, "':'")?;
            let dsf = self.alloc_box(
                DatumStructFrame {
                    start: span,
                    brace_start,
                    items,
                    key,
                    key_span,
                    max_child: 0,
                    qq_depth,
                    is_with: false,
                },
                span,
            )?;
            self.push_frame(ParseFrame::DatumStructFields(dsf), span)?;
            return Ok(NextStep::StartDatum {
                qq_depth,
                allow_splice: false,
            });
        }

        if is_upper && self.peek().kind == WorkTokenKind::Dot {
            self.advance()?;
            let (variant, vspan) = self.expect_ident("variant name after '.'")?;
            if self.peek().kind == WorkTokenKind::LParen && self.peek().attached {
                self.advance()?;
                self.push_frame(
                    ParseFrame::DatumVariantData {
                        span,
                        type_name: name,
                        variant,
                    },
                    span,
                )?;
                return Ok(NextStep::StartDatum {
                    qq_depth,
                    allow_splice: false,
                });
            }
            let variant_kw = self.intern_work_str("variant", span)?;
            let mut items = RawVec::new();
            for item in [
                Datum::new(span, DatumKind::Ident(variant_kw)),
                Datum::new(span, DatumKind::Ident(name)),
                Datum::new(vspan, DatumKind::Ident(variant)),
            ] {
                if items.push_work(self.work, self.core, item).is_none() {
                    return Err(self.oom(span));
                }
            }
            return Ok(NextStep::HaveDatum(
                Datum::new(span, DatumKind::List(items)),
                1,
                qq_depth,
            ));
        }

        Ok(NextStep::HaveDatum(
            Datum::new(span, DatumKind::Ident(name)),
            1,
            qq_depth,
        ))
    }

    pub(super) fn resume_datum_frame(
        &mut self,
        frame: ParseFrame,
        datum: Datum,
        depth: usize,
        qq_depth: usize,
    ) -> Result<NextStep, ErrRecorded> {
        match frame {
            ParseFrame::QuoteFinish {
                span,
                is_paren,
                is_quasiquote,
            } => {
                if is_paren {
                    self.expect(WorkTokenKind::RParen, "')'")?;
                }
                let boxed = self.alloc_box(datum, span)?;
                let kind = if is_quasiquote {
                    ExprKind::Quasiquote(boxed)
                } else {
                    ExprKind::Quote(boxed)
                };
                Ok(NextStep::HaveExpr(Expr::new(span, kind), 1 + depth))
            }
            ParseFrame::DatumWrap {
                span,
                tag,
                is_paren,
                outer_qq_depth,
            } => {
                if is_paren {
                    self.expect(WorkTokenKind::RParen, "')'")?;
                }
                let tag_str = self.intern_work_str(tag, span)?;
                let mut items = RawVec::new();
                for item in [Datum::new(span, DatumKind::Ident(tag_str)), datum] {
                    if items.push_work(self.work, self.core, item).is_none() {
                        return Err(self.oom(span));
                    }
                }
                Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::List(items)),
                    1 + depth,
                    outer_qq_depth,
                ))
            }
            ParseFrame::DatumList(mut dl) => {
                let dl_mut = dl.as_mut();
                if dl_mut
                    .items
                    .push_work(self.work, self.core, datum)
                    .is_none()
                {
                    return Err(self.oom(datum.span));
                }
                dl_mut.max_child = dl_mut.max_child.max(depth);
                let close = dl_mut.close;
                let start = dl_mut.start;
                let list_qq_depth = dl_mut.qq_depth;

                // `(with target { f1: v1 ... })` の糖衣正規化
                if close == WorkTokenKind::RParen
                    && dl_mut.items.len() == 2
                    && matches!(
                        dl_mut.items.as_slice()[0].kind,
                        DatumKind::Ident(s) if s.as_str() == "with"
                    )
                    && self.peek().kind == WorkTokenKind::LBrace
                {
                    let brace_start = self.peek().span;
                    self.advance()?;
                    let d = *dl.as_ref();
                    if self.peek().kind == WorkTokenKind::RBrace {
                        self.advance()?;
                        self.expect(WorkTokenKind::RParen, "')'")?;
                        return Ok(NextStep::HaveDatum(
                            Datum::new(d.start, DatumKind::List(d.items)),
                            1 + d.max_child,
                            d.qq_depth,
                        ));
                    }
                    let (key, key_span) = self.expect_ident("field name")?;
                    self.expect(WorkTokenKind::Colon, "':'")?;
                    let dsf = self.alloc_box(
                        DatumStructFrame {
                            start: d.start,
                            brace_start,
                            items: d.items,
                            key,
                            key_span,
                            max_child: d.max_child,
                            qq_depth: d.qq_depth,
                            is_with: true,
                        },
                        d.start,
                    )?;
                    self.push_frame(ParseFrame::DatumStructFields(dsf), d.start)?;
                    return Ok(NextStep::StartDatum {
                        qq_depth: d.qq_depth,
                        allow_splice: false,
                    });
                }

                if self.peek().kind == close {
                    self.advance()?;
                    let d = *dl.as_ref();
                    return Ok(NextStep::HaveDatum(
                        Datum::new(d.start, DatumKind::List(d.items)),
                        1 + d.max_child,
                        d.qq_depth,
                    ));
                }
                if self.at_eof() {
                    let msg = if close == WorkTokenKind::RBracket {
                        "unterminated '['"
                    } else {
                        "unterminated '('"
                    };
                    return Err(self.parse_err("src.syntax", start, msg));
                }
                self.push_frame(ParseFrame::DatumList(dl), start)?;
                Ok(NextStep::StartDatum {
                    qq_depth: list_qq_depth,
                    allow_splice: true,
                })
            }
            ParseFrame::DatumIndex {
                target,
                target_depth,
                qq_depth: idx_qq_depth,
            } => {
                self.expect(WorkTokenKind::RBracket, "']'")?;
                let combined = 1 + target_depth.max(depth);
                self.check_nesting(self.frames.len() + combined, target.span)?;
                let span = target.span;
                let index_kw = self.intern_work_str("index", span)?;
                let mut items = RawVec::new();
                for item in [Datum::new(span, DatumKind::Ident(index_kw)), *target, datum] {
                    if items.push_work(self.work, self.core, item).is_none() {
                        return Err(self.oom(span));
                    }
                }
                Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::List(items)),
                    combined,
                    idx_qq_depth,
                ))
            }
            ParseFrame::DatumVariantData {
                span,
                type_name,
                variant,
            } => {
                self.expect(WorkTokenKind::RParen, "')'")?;
                let variant_kw = self.intern_work_str("variant", span)?;
                let mut items = RawVec::new();
                for item in [
                    Datum::new(span, DatumKind::Ident(variant_kw)),
                    Datum::new(span, DatumKind::Ident(type_name)),
                    Datum::new(span, DatumKind::Ident(variant)),
                    datum,
                ] {
                    if items.push_work(self.work, self.core, item).is_none() {
                        return Err(self.oom(span));
                    }
                }
                Ok(NextStep::HaveDatum(
                    Datum::new(span, DatumKind::List(items)),
                    1 + depth,
                    qq_depth,
                ))
            }
            ParseFrame::DatumStructFields(mut dsf) => {
                let key = dsf.as_ref().key;
                let key_span = dsf.as_ref().key_span;
                let mut pair = RawVec::new();
                for item in [Datum::new(key_span, DatumKind::Ident(key)), datum] {
                    if pair.push_work(self.work, self.core, item).is_none() {
                        return Err(self.oom(key_span));
                    }
                }
                let pair_datum = Datum::new(key_span, DatumKind::List(pair));
                let dsf_mut = dsf.as_mut();
                if dsf_mut
                    .items
                    .push_work(self.work, self.core, pair_datum)
                    .is_none()
                {
                    return Err(self.oom(key_span));
                }
                dsf_mut.max_child = dsf_mut.max_child.max(1 + depth);
                if self.peek().kind == WorkTokenKind::RBrace {
                    self.advance()?;
                    let d = *dsf.as_ref();
                    if d.is_with {
                        self.expect(WorkTokenKind::RParen, "')'")?;
                    }
                    return Ok(NextStep::HaveDatum(
                        Datum::new(d.start, DatumKind::List(d.items)),
                        1 + d.max_child,
                        d.qq_depth,
                    ));
                }
                if self.at_eof() {
                    return Err(self.parse_err(
                        "src.syntax",
                        dsf.as_ref().brace_start,
                        "unterminated '{'",
                    ));
                }
                let (next_key, next_key_span) = self.expect_ident("field name")?;
                self.expect(WorkTokenKind::Colon, "':'")?;
                let dsf_mut = dsf.as_mut();
                dsf_mut.key = next_key;
                dsf_mut.key_span = next_key_span;
                let sp = dsf_mut.start;
                let field_qq_depth = dsf_mut.qq_depth;
                self.push_frame(ParseFrame::DatumStructFields(dsf), sp)?;
                Ok(NextStep::StartDatum {
                    qq_depth: field_qq_depth,
                    allow_splice: false,
                })
            }
            _ => unreachable!("unexpected non-datum frame in resume_datum_frame"),
        }
    }
}
