//! 静的型検査器と永続型環境 (`TypeEnv`)。
//!
//! - 確定済みの型定義・関数シグネチャ・モジュール公開型は `PermArena` に保存して凍結する。
//! - 型推論・単一化・網羅性検査の一時データはすべて `WorkArena` 上で処理し、
//!   システムヒープ確保を行わず `memory_bytes` に計上する。

// STEP 3 の評価パイプライン統合まではテストからのみ参照される項目があるため抑制。
#![allow(dead_code, unused_imports)]

mod api;
mod boundary;
mod builtins;
mod checker;
#[cfg(test)]
mod tests;
mod types;
mod work_types;

pub use api::{FnSigRef, TypeHandle, TypeRef};

pub(crate) use boundary::{
    check_rval_against_type, check_rval_against_type_expr, reject_fn_in_perm_type,
    validate_boundary_type_expr,
};
pub(crate) use checker::{CheckOptions, check_program_work, resolve_type_expr_perm};
pub(crate) use types::{
    FnSig, ModuleTypeEnv, PermTypeNode, SchemaDef, SchemaId, SchemaKind, TY_BOOL, TY_CHAR,
    TY_DATUM, TY_F64, TY_I64, TY_IDENT, TY_TEXT, TY_VOID, TypeEnv, TypeEnvSnapshot, TypeId,
    ValBinding, VariantCase,
};
pub(crate) use work_types::TYPECHECK_WORK_MULTIPLIER;
