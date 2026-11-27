//! 型式（`TypeExpr`）および `defschema` の非再帰構文解析（`WorkArena` 完結）。
use super::*;

#[derive(Clone, Copy)]
enum TypeFrame {
    List {
        start: Span,
    },
    Array {
        start: Span,
    },
    Option {
        start: Span,
    },
    ResultOk {
        start: Span,
    },
    ResultErr {
        start: Span,
        ok: WorkBox<TypeExpr>,
        ok_depth: usize,
    },
    Tuple {
        start: Span,
        items: RawVec<TypeExpr>,
        max_child: usize,
    },
    FnParams {
        start: Span,
        param_start: Span,
        params: RawVec<TypeExpr>,
        max_child: usize,
    },
    FnRet {
        start: Span,
        params: RawVec<TypeExpr>,
        max_child: usize,
    },
}

enum TypeStep {
    Start,
    Have(TypeExpr, usize),
}

fn is_upper_ident(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

impl Parser<'_, '_, '_> {
    /// 型式 `TypeExpr` を非再帰で解析し、`(TypeExpr, 構文深さ)` を返す。
    pub(super) fn parse_type_expr(
        &mut self,
        base_depth: usize,
    ) -> Result<(TypeExpr, usize), ErrRecorded> {
        let mut stack = RawVec::<TypeFrame>::new();
        let mut step = TypeStep::Start;

        loop {
            match step {
                TypeStep::Start => {
                    let cur_depth = base_depth + stack.len();
                    self.check_nesting(cur_depth + 1, self.peek().span)?;
                    let tok = self.peek();
                    let span = tok.span;
                    match tok.kind {
                        WorkTokenKind::Ident(name) => {
                            self.advance()?;
                            let kind = match name.as_str() {
                                "void" => TypeExprKind::Void,
                                "i64" => TypeExprKind::I64,
                                "f64" => TypeExprKind::F64,
                                "bool" => TypeExprKind::Bool,
                                "char" => TypeExprKind::Char,
                                "text" => TypeExprKind::Text,
                                "ident" => TypeExprKind::Ident,
                                "datum" => TypeExprKind::Datum,
                                "any" => {
                                    return Err(self.parse_err(
                                        "type.expr",
                                        span,
                                        "'any' is not a valid type",
                                    ));
                                }
                                s if is_upper_ident(s) => {
                                    TypeExprKind::Named { module: None, name }
                                }
                                _ => {
                                    if self.peek().kind == WorkTokenKind::Dot {
                                        self.advance()?;
                                        let (ty_name, ty_span) =
                                            self.expect_ident("type name after '.'")?;
                                        if !is_upper_ident(ty_name.as_str()) {
                                            return Err(self.parse_err_fmt(
                                                "type.expr",
                                                ty_span,
                                                format_args!(
                                                    "type name '{}' must start with an uppercase letter",
                                                    ty_name.as_str()
                                                ),
                                            ));
                                        }
                                        TypeExprKind::Named {
                                            module: Some(name),
                                            name: ty_name,
                                        }
                                    } else {
                                        return Err(self.parse_err_fmt(
                                            "type.expr",
                                            span,
                                            format_args!("unknown type '{}'", name.as_str()),
                                        ));
                                    }
                                }
                            };
                            step = TypeStep::Have(TypeExpr::new(span, kind), 1);
                        }
                        WorkTokenKind::LParen => {
                            self.advance()?;
                            let (head, hspan) = self.expect_ident("type constructor")?;
                            let frame = match head.as_str() {
                                "list" => TypeFrame::List { start: span },
                                "array" => TypeFrame::Array { start: span },
                                "option" => TypeFrame::Option { start: span },
                                "result" => TypeFrame::ResultOk { start: span },
                                "tuple" => {
                                    if self.peek().kind == WorkTokenKind::RParen {
                                        return Err(self.parse_err(
                                            "schema.tuple",
                                            span,
                                            "tuple type requires at least 2 elements",
                                        ));
                                    }
                                    TypeFrame::Tuple {
                                        start: span,
                                        items: RawVec::new(),
                                        max_child: 0,
                                    }
                                }
                                "fn" => {
                                    let param_start = self
                                        .expect(WorkTokenKind::LParen, "fn parameter types '('")?;
                                    if self.peek().kind == WorkTokenKind::RParen {
                                        self.advance()?;
                                        TypeFrame::FnRet {
                                            start: span,
                                            params: RawVec::new(),
                                            max_child: 0,
                                        }
                                    } else {
                                        TypeFrame::FnParams {
                                            start: span,
                                            param_start,
                                            params: RawVec::new(),
                                            max_child: 0,
                                        }
                                    }
                                }
                                other => {
                                    return Err(self.parse_err_fmt(
                                        "type.expr",
                                        hspan,
                                        format_args!("unknown type constructor '{other}'"),
                                    ));
                                }
                            };
                            if stack.push_work(self.work, self.core, frame).is_none() {
                                return Err(self.oom(span));
                            }
                            step = TypeStep::Start;
                        }
                        _ => {
                            return Err(self.parse_err(
                                "type.expr",
                                span,
                                "expected type expression",
                            ));
                        }
                    }
                }
                TypeStep::Have(ty, depth) => {
                    let Some(frame) = stack.pop() else {
                        return Ok((ty, depth));
                    };
                    match frame {
                        TypeFrame::List { start } => {
                            self.expect(WorkTokenKind::RParen, "')'")?;
                            let inner = self.alloc_box(ty, start)?;
                            let total = 1 + depth;
                            self.check_nesting(base_depth + stack.len() + total, start)?;
                            step = TypeStep::Have(
                                TypeExpr::new(start, TypeExprKind::List(inner)),
                                total,
                            );
                        }
                        TypeFrame::Array { start } => {
                            self.expect(WorkTokenKind::RParen, "')'")?;
                            let inner = self.alloc_box(ty, start)?;
                            let total = 1 + depth;
                            self.check_nesting(base_depth + stack.len() + total, start)?;
                            step = TypeStep::Have(
                                TypeExpr::new(start, TypeExprKind::Array(inner)),
                                total,
                            );
                        }
                        TypeFrame::Option { start } => {
                            self.expect(WorkTokenKind::RParen, "')'")?;
                            let inner = self.alloc_box(ty, start)?;
                            let total = 1 + depth;
                            self.check_nesting(base_depth + stack.len() + total, start)?;
                            step = TypeStep::Have(
                                TypeExpr::new(start, TypeExprKind::Option(inner)),
                                total,
                            );
                        }
                        TypeFrame::ResultOk { start } => {
                            let ok = self.alloc_box(ty, start)?;
                            if stack
                                .push_work(
                                    self.work,
                                    self.core,
                                    TypeFrame::ResultErr {
                                        start,
                                        ok,
                                        ok_depth: depth,
                                    },
                                )
                                .is_none()
                            {
                                return Err(self.oom(start));
                            }
                            step = TypeStep::Start;
                        }
                        TypeFrame::ResultErr {
                            start,
                            ok,
                            ok_depth,
                        } => {
                            self.expect(WorkTokenKind::RParen, "')'")?;
                            let err = self.alloc_box(ty, start)?;
                            let total = 1 + ok_depth.max(depth);
                            self.check_nesting(base_depth + stack.len() + total, start)?;
                            step = TypeStep::Have(
                                TypeExpr::new(start, TypeExprKind::Result { ok, err }),
                                total,
                            );
                        }
                        TypeFrame::Tuple {
                            start,
                            mut items,
                            mut max_child,
                        } => {
                            if items.push_work(self.work, self.core, ty).is_none() {
                                return Err(self.oom(ty.span));
                            }
                            max_child = max_child.max(depth);
                            if self.peek().kind == WorkTokenKind::RParen {
                                if items.len() < 2 {
                                    return Err(self.parse_err(
                                        "schema.tuple",
                                        start,
                                        "tuple type requires at least 2 elements",
                                    ));
                                }
                                self.advance()?;
                                let total = 1 + max_child;
                                self.check_nesting(base_depth + stack.len() + total, start)?;
                                step = TypeStep::Have(
                                    TypeExpr::new(start, TypeExprKind::Tuple(items)),
                                    total,
                                );
                            } else if self.at_eof() {
                                return Err(self.parse_err(
                                    "src.syntax",
                                    start,
                                    "unterminated tuple type '('",
                                ));
                            } else {
                                if stack
                                    .push_work(
                                        self.work,
                                        self.core,
                                        TypeFrame::Tuple {
                                            start,
                                            items,
                                            max_child,
                                        },
                                    )
                                    .is_none()
                                {
                                    return Err(self.oom(start));
                                }
                                step = TypeStep::Start;
                            }
                        }
                        TypeFrame::FnParams {
                            start,
                            param_start,
                            mut params,
                            mut max_child,
                        } => {
                            if params.push_work(self.work, self.core, ty).is_none() {
                                return Err(self.oom(ty.span));
                            }
                            max_child = max_child.max(depth);
                            if self.peek().kind == WorkTokenKind::RParen {
                                self.advance()?;
                                if stack
                                    .push_work(
                                        self.work,
                                        self.core,
                                        TypeFrame::FnRet {
                                            start,
                                            params,
                                            max_child,
                                        },
                                    )
                                    .is_none()
                                {
                                    return Err(self.oom(start));
                                }
                                step = TypeStep::Start;
                            } else if self.at_eof() {
                                return Err(self.parse_err(
                                    "src.syntax",
                                    param_start,
                                    "unterminated fn parameter type list '('",
                                ));
                            } else {
                                if stack
                                    .push_work(
                                        self.work,
                                        self.core,
                                        TypeFrame::FnParams {
                                            start,
                                            param_start,
                                            params,
                                            max_child,
                                        },
                                    )
                                    .is_none()
                                {
                                    return Err(self.oom(start));
                                }
                                step = TypeStep::Start;
                            }
                        }
                        TypeFrame::FnRet {
                            start,
                            params,
                            max_child,
                        } => {
                            self.expect(WorkTokenKind::RParen, "')'")?;
                            let ret = self.alloc_box(ty, start)?;
                            let total = 1 + max_child.max(depth);
                            self.check_nesting(base_depth + stack.len() + total, start)?;
                            step = TypeStep::Have(
                                TypeExpr::new(start, TypeExprKind::Fn { params, ret }),
                                total,
                            );
                        }
                    }
                }
            }
        }
    }

    /// `(defschema Name SchemaBody)` を解析する（`defschema` トークンは呼び出し元で消費済み）。
    pub(super) fn parse_defschema(&mut self, start: Span) -> Result<(Expr, usize), ErrRecorded> {
        let base_depth = self.frames.len() + 1;
        let (name, nspan) = self.expect_ident("schema type name")?;
        if !is_upper_ident(name.as_str()) {
            return Err(self.parse_err_fmt(
                "type.defschema",
                nspan,
                format_args!(
                    "schema name '{}' must start with an uppercase letter",
                    name.as_str()
                ),
            ));
        }

        let (body, body_depth) = if self.peek().kind == WorkTokenKind::LParen {
            let bstart = self.peek().span;
            self.advance()?;
            let (kind_kw, kspan) = self.expect_ident("'struct', 'open_struct', or 'variant'")?;
            match kind_kw.as_str() {
                "struct" | "open_struct" => {
                    let open = kind_kw.as_str() == "open_struct";
                    let (type_name, tspan) = self.expect_ident("struct type name")?;
                    if !is_upper_ident(type_name.as_str()) {
                        return Err(self.parse_err_fmt(
                            "type.defschema",
                            tspan,
                            format_args!(
                                "struct name '{}' must start with an uppercase letter",
                                type_name.as_str()
                            ),
                        ));
                    }
                    let mut fields = RawVec::new();
                    let mut max_child = 0usize;
                    while self.peek().kind != WorkTokenKind::RParen {
                        if self.at_eof() {
                            return Err(self.parse_err(
                                "src.syntax",
                                bstart,
                                "unterminated struct schema '('",
                            ));
                        }
                        let fstart = self.expect(WorkTokenKind::LParen, "field definition '('")?;
                        let (fname, _) = self.expect_ident("field name")?;
                        let (fty, fdepth) = self.parse_type_expr(base_depth + 1)?;
                        self.expect(WorkTokenKind::RParen, "')'")?;
                        max_child = max_child.max(fdepth);
                        if fields
                            .push_work(self.work, self.core, (fname, fty))
                            .is_none()
                        {
                            return Err(self.oom(fstart));
                        }
                    }
                    self.advance()?; // )
                    (
                        SchemaBody::Struct {
                            open,
                            type_name,
                            fields,
                        },
                        1 + max_child,
                    )
                }
                "variant" => {
                    let (type_name, tspan) = self.expect_ident("variant type name")?;
                    if !is_upper_ident(type_name.as_str()) {
                        return Err(self.parse_err_fmt(
                            "type.defschema",
                            tspan,
                            format_args!(
                                "variant type name '{}' must start with an uppercase letter",
                                type_name.as_str()
                            ),
                        ));
                    }
                    let mut cases = RawVec::new();
                    let mut max_child = 0usize;
                    while self.peek().kind != WorkTokenKind::RParen {
                        if self.at_eof() {
                            return Err(self.parse_err(
                                "src.syntax",
                                bstart,
                                "unterminated variant schema '('",
                            ));
                        }
                        let cstart =
                            self.expect(WorkTokenKind::LParen, "variant case definition '('")?;
                        let (cname, cspan) = self.expect_ident("variant case name")?;
                        if !is_upper_ident(cname.as_str()) {
                            return Err(self.parse_err_fmt(
                                "type.defschema",
                                cspan,
                                format_args!(
                                    "variant case '{}' must start with an uppercase letter",
                                    cname.as_str()
                                ),
                            ));
                        }
                        let payload = if self.peek().kind == WorkTokenKind::RParen {
                            None
                        } else {
                            let (pty, pdepth) = self.parse_type_expr(base_depth + 1)?;
                            max_child = max_child.max(pdepth);
                            Some(self.alloc_box(pty, pty.span)?)
                        };
                        self.expect(WorkTokenKind::RParen, "')'")?;
                        if cases
                            .push_work(
                                self.work,
                                self.core,
                                VariantCaseDef {
                                    span: cstart,
                                    name: cname,
                                    payload,
                                },
                            )
                            .is_none()
                        {
                            return Err(self.oom(cstart));
                        }
                    }
                    self.advance()?; // )
                    (SchemaBody::Variant { type_name, cases }, 1 + max_child)
                }
                other => {
                    return Err(self.parse_err_fmt(
                        "type.defschema",
                        kspan,
                        format_args!(
                            "expected 'struct', 'open_struct', or 'variant' in defschema, got '{other}'"
                        ),
                    ));
                }
            }
        } else {
            // 既存の struct / open_struct / variant 型名（`Name` または `mod.Name`）からの別型定義
            let (first, fspan) = self.expect_ident("schema body or existing type name")?;
            let body = if is_upper_ident(first.as_str()) {
                SchemaBody::CloneOf {
                    module: None,
                    name: first,
                }
            } else if self.peek().kind == WorkTokenKind::Dot {
                self.advance()?;
                let (ty_name, ty_span) = self.expect_ident("type name after '.'")?;
                if !is_upper_ident(ty_name.as_str()) {
                    return Err(self.parse_err_fmt(
                        "type.defschema",
                        ty_span,
                        format_args!(
                            "type name '{}' must start with an uppercase letter",
                            ty_name.as_str()
                        ),
                    ));
                }
                SchemaBody::CloneOf {
                    module: Some(first),
                    name: ty_name,
                }
            } else {
                return Err(self.parse_err_fmt(
                    "type.defschema",
                    fspan,
                    format_args!(
                        "expected '(struct ...)', '(open_struct ...)', '(variant ...)', or type name in defschema, got '{}'",
                        first.as_str()
                    ),
                ));
            };
            (body, 1)
        };

        self.expect(WorkTokenKind::RParen, "')'")?;
        let ds = self.alloc_box(DefSchemaExpr { name, body }, start)?;
        Ok((Expr::new(start, ExprKind::DefSchema(ds)), 1 + body_depth))
    }
}
