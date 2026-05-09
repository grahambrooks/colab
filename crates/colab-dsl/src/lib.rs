//! Codemod script DSL: parsing, compilation, and runtime IR.
//!
//! Callers compile a script with [`compile`] (or use [`parse`] for the
//! raw AST) and hand the resulting [`Refactoring`] to
//! [`colab_core::walker`]. [`compile`] takes a
//! [`colab_core::BackendRegistry`] so this crate has no compile-time
//! knowledge of which language backends exist.

pub mod ast;
pub mod compiler;
pub mod model;

pub use compiler::{compile, parse};
pub use model::Refactoring;

// Re-export so consumers can use the trait without depending on
// colab-core directly.
pub use colab_core::CodeTransformer;
