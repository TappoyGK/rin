//! スキーマのDebug表示。深い型は反復処理で出力する。
use super::{Schema, types::TypeExpr};
use std::{collections::BTreeMap, fmt};

impl TypeExpr {
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
                    TypeExpr::Ident => f.write_str("Ident")?,
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

impl Schema {
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
}
