//! 深いスキーマの複製。
use super::{
    Schema,
    types::{TypeExpr, UnaryTypeKind},
};

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
                    TypeExpr::Datum => next = CloneNext::Have(TypeExpr::Datum),
                    TypeExpr::Void => next = CloneNext::Have(TypeExpr::Void),
                    TypeExpr::I64 => next = CloneNext::Have(TypeExpr::I64),
                    TypeExpr::F64 => next = CloneNext::Have(TypeExpr::F64),
                    TypeExpr::Bool => next = CloneNext::Have(TypeExpr::Bool),
                    TypeExpr::Char => next = CloneNext::Have(TypeExpr::Char),
                    TypeExpr::Text => next = CloneNext::Have(TypeExpr::Text),
                    TypeExpr::Ident => next = CloneNext::Have(TypeExpr::Ident),
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
}
