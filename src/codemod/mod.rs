//! Codemod script DSL: parsing, compilation, and runtime IR.
//!
//! The public surface is small: callers compile a script with [`compile`]
//! (or use [`parse`] for the raw AST) and then hand the resulting
//! [`Refactoring`] to the [`crate::walker`] to rewrite files.

pub mod ast;
pub mod compiler;
mod go;
pub mod model;

pub use compiler::compile;
pub use model::CodeTransformer;
