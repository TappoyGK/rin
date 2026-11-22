//! 組み込み関数および高階リスト・配列演算を実装する。

mod callbacks;
mod collections;

use crate::bytecode::BuiltinId;
use crate::error::{Error, Span};
use crate::pool::{Obj, RVal};

use super::{CallOrigin, Runner};

#[cfg(debug_assertions)]
impl BuiltinId {
    /// Rin の関数を呼び戻す組み込み関数。Debug では命令ループを抜けてから実行する。
    #[inline(always)]
    pub(super) fn has_callback(self) -> bool {
        matches!(
            self,
            Self::Find
                | Self::Any
                | Self::All
                | Self::Map
                | Self::Filter
                | Self::FlatMap
                | Self::Fold
                | Self::SortBy
        )
    }
}

impl Runner<'_, '_> {
    // Debug では高階関数の再入時に通常の組み込み命令の作業領域を残さない。
    // Release は apply_builtin への単純な委譲。大きな本体は VM にインライン化しない。
    #[inline(always)]
    pub(super) fn dispatch_builtin(
        &mut self,
        b: BuiltinId,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, Error> {
        #[cfg(not(debug_assertions))]
        return self.apply_builtin(b, args, span);
        #[cfg(debug_assertions)]
        match b {
            BuiltinId::Find => self.builtin_find(args, span),
            BuiltinId::Any => self.builtin_any(args, span),
            BuiltinId::All => self.builtin_all(args, span),
            BuiltinId::Map => self.builtin_map(args, span),
            BuiltinId::Filter => self.builtin_filter(args, span),
            BuiltinId::FlatMap => self.builtin_flat_map(args, span),
            BuiltinId::Fold => self.builtin_fold(args, span),
            BuiltinId::SortBy => self.builtin_sort_by(args, span),
            _ => self.apply_builtin(b, args, span),
        }
    }

    fn apply_builtin(&mut self, b: BuiltinId, args: &[RVal], span: Span) -> Result<RVal, Error> {
        match b {
            BuiltinId::Add | BuiltinId::Sub | BuiltinId::Mul | BuiltinId::Div | BuiltinId::Rem => {
                self.arith(b, args, span)
            }
            BuiltinId::Eq | BuiltinId::Ne => self.eq_cmp(b == BuiltinId::Eq, args, span),
            BuiltinId::Lt | BuiltinId::Le | BuiltinId::Gt | BuiltinId::Ge => {
                self.ord_cmp(b, args, span)
            }
            BuiltinId::And => {
                if args.is_empty() {
                    return Err(Error::runtime(
                        "op.logic",
                        span,
                        "'and' requires at least 1 argument",
                    ));
                }
                for v in args {
                    match v {
                        RVal::Bool(true) => {}
                        RVal::Bool(false) => return Ok(RVal::Bool(false)),
                        other => {
                            return Err(Error::runtime(
                                "op.logic",
                                span,
                                format!("expected bool, got {}", other.type_name(self.pool)),
                            ));
                        }
                    }
                }
                Ok(RVal::Bool(true))
            }
            BuiltinId::Or => {
                if args.is_empty() {
                    return Err(Error::runtime(
                        "op.logic",
                        span,
                        "'or' requires at least 1 argument",
                    ));
                }
                for v in args {
                    match v {
                        RVal::Bool(true) => return Ok(RVal::Bool(true)),
                        RVal::Bool(false) => {}
                        other => {
                            return Err(Error::runtime(
                                "op.logic",
                                span,
                                format!("expected bool, got {}", other.type_name(self.pool)),
                            ));
                        }
                    }
                }
                Ok(RVal::Bool(false))
            }
            BuiltinId::Not => {
                let [v] = args else {
                    return Err(Error::runtime("op.logic", span, "'not' takes 1 argument"));
                };
                match v {
                    RVal::Bool(b) => Ok(RVal::Bool(!b)),
                    other => Err(Error::runtime(
                        "op.logic",
                        span,
                        format!("'not' expects bool, got {}", other.type_name(self.pool)),
                    )),
                }
            }
            BuiltinId::AsF64 => {
                let [v] = args else {
                    return Err(Error::runtime(
                        "fn.convert",
                        span,
                        "'as_f64' takes 1 argument",
                    ));
                };
                match v {
                    RVal::Int(n) => Ok(RVal::Float(*n as f64)),
                    RVal::Float(x) => Ok(RVal::Float(*x)),
                    other => Err(Error::runtime(
                        "fn.convert",
                        span,
                        format!(
                            "'as_f64' expects number, got {}",
                            other.type_name(self.pool)
                        ),
                    )),
                }
            }
            BuiltinId::AsI64 => {
                let [v] = args else {
                    return Err(Error::runtime(
                        "fn.convert",
                        span,
                        "'as_i64' takes 1 argument",
                    ));
                };
                match v {
                    RVal::Int(n) => Ok(RVal::Int(*n)),
                    RVal::Float(x) => {
                        if !x.is_finite() || *x < (i64::MIN as f64) || *x > (i64::MAX as f64) {
                            return Err(Error::runtime(
                                "fn.convert",
                                span,
                                "float out of i64 range",
                            ));
                        }
                        Ok(RVal::Int(*x as i64))
                    }
                    other => Err(Error::runtime(
                        "fn.convert",
                        span,
                        format!(
                            "'as_i64' expects number, got {}",
                            other.type_name(self.pool)
                        ),
                    )),
                }
            }
            BuiltinId::Sin
            | BuiltinId::Cos
            | BuiltinId::Sqrt
            | BuiltinId::Abs
            | BuiltinId::Floor
            | BuiltinId::Ceil => self.math(b, args, span),
            BuiltinId::List => self.build_list_from_borrowed(args, span),
            BuiltinId::Len => self.builtin_len(args, span),
            BuiltinId::IsEmpty => self.builtin_is_empty(args, span),
            BuiltinId::Get => self.builtin_get(args, span),
            BuiltinId::Car => self.builtin_car(args, span),
            BuiltinId::Cdr => self.builtin_cdr(args, span),
            BuiltinId::Last => self.builtin_last(args, span),
            BuiltinId::Cons => self.builtin_cons(args, span),
            BuiltinId::Push => self.builtin_push(args, span),
            BuiltinId::Append => self.builtin_append(args, span),
            BuiltinId::Flatten => self.builtin_flatten(args, span),
            BuiltinId::Slice => self.builtin_slice(args, span),
            BuiltinId::Take => self.builtin_take(args, span),
            BuiltinId::Drop => self.builtin_drop(args, span),
            BuiltinId::Reverse => self.builtin_reverse(args, span),
            BuiltinId::Range => self.builtin_range(args, span),
            BuiltinId::Contains => self.builtin_contains(args, span),
            BuiltinId::Find => self.builtin_find(args, span),
            BuiltinId::Any => self.builtin_any(args, span),
            BuiltinId::All => self.builtin_all(args, span),
            BuiltinId::Map => self.builtin_map(args, span),
            BuiltinId::Filter => self.builtin_filter(args, span),
            BuiltinId::FlatMap => self.builtin_flat_map(args, span),
            BuiltinId::Fold => self.builtin_fold(args, span),
            BuiltinId::Zip => self.builtin_zip(args, span),
            BuiltinId::Enumerate => self.builtin_enumerate(args, span),
            BuiltinId::Sort => self.builtin_sort(args, span),
            BuiltinId::SortBy => self.builtin_sort_by(args, span),
            BuiltinId::Unique => self.builtin_unique(args, span),
            BuiltinId::ListToArray => self.builtin_list_to_array(args, span),
            BuiltinId::ArrayToList => self.builtin_array_to_list(args, span),
        }
    }
}
