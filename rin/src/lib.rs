//! 純粋関数型 Lisp 系言語 Rin の処理系。

pub mod alloc;
mod ast;
mod bytecode;
mod compiler;
mod diagnostic;
pub mod error;
mod eval;
pub mod ffi;
pub mod fmt;
mod lexer;
pub mod math;
#[allow(dead_code)]
mod mem;
mod parser;
mod pool;
pub mod schema;
pub mod test;
pub mod value;
mod work;

pub use alloc::{Allocator, ArenaAllocator, HeapAllocator};
pub use error::{Error, ErrorKind, Limits, Span, StackFrame};
pub use eval::{Builder, Machine, eval_with, eval_with_allocator};
pub use math::MathFma;
pub use mem::{MemoryConfig, RegionConfig};
pub use schema::Schema;
pub use value::Value;

/// 既定の上限値・外部束縛なしで Rin ソースを評価する。
pub fn eval(source: &str) -> Result<Value, Error> {
    eval_with(source, &[], Limits::default())
}

#[cfg(test)]
mod tests;
