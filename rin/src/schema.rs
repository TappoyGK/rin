//! Rinの値構造を検証するスキーマ。

mod check;
mod clone;
mod drop;
mod format;
mod parser;
mod types;

use crate::{Error, Value};
use std::collections::BTreeMap;
pub(crate) use types::TypeExpr;

/// `Value` の構造を検証するコンパイル済み Rin スキーマ。
///
/// 深い入れ子（深さ 32 以上）を含むスキーマでは、`Clone`・`Drop`・`Debug` を反復処理へ切り替えるため、
/// `{:#?}` 出力は 1 行形式となる。
pub struct Schema {
    defs: BTreeMap<String, TypeExpr>,
    root: TypeExpr,
    has_deep: bool,
}

impl Schema {
    /// `value` がこのスキーマに適合するか検証する。
    pub fn check(&self, value: &Value) -> Result<(), Error> {
        check::check(self, value)
    }
}
