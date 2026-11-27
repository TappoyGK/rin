//! 純粋関数型 Lisp 系言語 Rin の処理系。

pub mod alloc;
mod ast;
mod bytecode;
mod compiler;
mod diagnostic;
pub mod error;
mod eval;
pub mod ffi;
#[cfg(feature = "std-value")]
pub mod fmt;
mod lexer;
pub mod math;
#[allow(dead_code)]
mod mem;
mod parser;
mod pool;
#[cfg(feature = "std-value")]
pub mod schema;
#[cfg(feature = "std-value")]
pub mod test;
mod typecheck;
#[cfg(feature = "std-value")]
pub mod value;
pub mod value_ref;
#[cfg(feature = "std-value")]
mod work;

pub use alloc::{Allocator, ArenaAllocator, HeapAllocator};
#[cfg(feature = "std-value")]
pub use error::{Error, StackFrame};
pub use error::{ErrorKind, ErrorRef, Limits, Span};
pub use eval::{Builder, Machine};
#[cfg(feature = "std-value")]
pub use eval::{eval_with, eval_with_allocator};
pub use math::MathFma;
pub use mem::{MemoryConfig, RegionConfig};
#[cfg(feature = "std-value")]
pub use schema::Schema;
pub use typecheck::{FnSigRef, TypeHandle, TypeRef};
#[cfg(feature = "std-value")]
pub use value::Value;
pub use value_ref::{ListIter, SliceView, StructRef, Val, ValueRef, ValueTag, VariantRef};

/// 既定の上限値・外部束縛なしで Rin ソースを評価する。
#[cfg(feature = "std-value")]
pub fn eval(source: &str) -> Result<Value, Error> {
    eval_with(source, &[], Limits::default())
}

#[cfg(all(test, feature = "std-value"))]
mod tests;
