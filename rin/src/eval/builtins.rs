//! 組み込み関数および高階リスト・配列演算を実装する。

pub(super) mod callbacks;
mod collections;

use crate::bytecode::BuiltinId;
use crate::error::{ErrRecorded, Span};
use crate::pool::{Obj, RVal};

use super::Runner;

impl BuiltinId {
    /// Rin の関数を呼び戻す高階組み込み関数かどうか。
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
    /// コールバックを持たない組み込み関数を実行する（ステップ 1 は呼び出し元で計上済み）。
    pub(super) fn apply_non_callback_builtin(
        &mut self,
        b: BuiltinId,
        args: &[RVal],
        span: Span,
    ) -> Result<RVal, ErrRecorded> {
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
                    return Err(self.err_runtime(
                        "op.logic",
                        span,
                        "'and' requires at least 1 argument",
                    ));
                }
                for v in args {
                    if !self.try_charge(1) {
                        return self.step_limit_result(span);
                    }
                    match v {
                        RVal::Bool(true) => {}
                        RVal::Bool(false) => return Ok(RVal::Bool(false)),
                        other => {
                            let tname = other.type_name(self.pool);
                            return Err(self.err_runtime_fmt(
                                "op.logic",
                                span,
                                format_args!("expected bool, got {tname}"),
                            ));
                        }
                    }
                }
                Ok(RVal::Bool(true))
            }
            BuiltinId::Or => {
                if args.is_empty() {
                    return Err(self.err_runtime(
                        "op.logic",
                        span,
                        "'or' requires at least 1 argument",
                    ));
                }
                for v in args {
                    if !self.try_charge(1) {
                        return self.step_limit_result(span);
                    }
                    match v {
                        RVal::Bool(true) => return Ok(RVal::Bool(true)),
                        RVal::Bool(false) => {}
                        other => {
                            let tname = other.type_name(self.pool);
                            return Err(self.err_runtime_fmt(
                                "op.logic",
                                span,
                                format_args!("expected bool, got {tname}"),
                            ));
                        }
                    }
                }
                Ok(RVal::Bool(false))
            }
            BuiltinId::Not => {
                let [v] = args else {
                    return Err(self.err_runtime("op.logic", span, "'not' takes 1 argument"));
                };
                match v {
                    RVal::Bool(b) => Ok(RVal::Bool(!b)),
                    other => {
                        let tname = other.type_name(self.pool);
                        Err(self.err_runtime_fmt(
                            "op.logic",
                            span,
                            format_args!("'not' expects bool, got {tname}"),
                        ))
                    }
                }
            }
            BuiltinId::AsF64 => {
                let [v] = args else {
                    return Err(self.err_runtime("fn.convert", span, "'as_f64' takes 1 argument"));
                };
                match v {
                    RVal::Int(n) => Ok(RVal::Float(*n as f64)),
                    RVal::Float(x) => Ok(RVal::Float(*x)),
                    other => {
                        let tname = other.type_name(self.pool);
                        Err(self.err_runtime_fmt(
                            "fn.convert",
                            span,
                            format_args!("'as_f64' expects number, got {tname}"),
                        ))
                    }
                }
            }
            BuiltinId::AsI64 => {
                let [v] = args else {
                    return Err(self.err_runtime("fn.convert", span, "'as_i64' takes 1 argument"));
                };
                match v {
                    RVal::Int(n) => Ok(RVal::Int(*n)),
                    RVal::Float(x) => {
                        if !x.is_finite() || *x < (i64::MIN as f64) || *x > (i64::MAX as f64) {
                            return Err(self.err_runtime(
                                "fn.convert",
                                span,
                                "float out of i64 range",
                            ));
                        }
                        Ok(RVal::Int(*x as i64))
                    }
                    other => {
                        let tname = other.type_name(self.pool);
                        Err(self.err_runtime_fmt(
                            "fn.convert",
                            span,
                            format_args!("'as_i64' expects number, got {tname}"),
                        ))
                    }
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
            BuiltinId::Zip => self.builtin_zip(args, span),
            BuiltinId::Enumerate => self.builtin_enumerate(args, span),
            BuiltinId::Sort => self.builtin_sort(args, span),
            BuiltinId::Unique => self.builtin_unique(args, span),
            BuiltinId::ListToArray => self.builtin_list_to_array(args, span),
            BuiltinId::ArrayToList => self.builtin_array_to_list(args, span),
            BuiltinId::Find
            | BuiltinId::Any
            | BuiltinId::All
            | BuiltinId::Map
            | BuiltinId::Filter
            | BuiltinId::FlatMap
            | BuiltinId::Fold
            | BuiltinId::SortBy => {
                unreachable!("callback builtins are handled by the VM frame loop")
            }
        }
    }
}
