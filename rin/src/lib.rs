pub mod alloc;
mod ast;
mod bytecode;
mod compiler;
pub mod error;
mod eval;
pub mod ffi;
mod lexer;
mod parser;
mod pool;
pub mod schema;
pub mod value;

pub use alloc::{Allocator, ArenaAllocator, HeapAllocator};
pub use error::{Error, ErrorKind, Limits, Span};
pub use eval::{Machine, eval_with, eval_with_allocator};
pub use schema::Schema;
pub use value::Value;

/// Evaluates Rin source code with default limits and no external bindings.
pub fn eval(source: &str) -> Result<Value, Error> {
    eval_with(source, &[], Limits::default())
}

#[cfg(test)]
mod tests;
