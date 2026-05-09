//! Go-specific source transformations.
//!
//! These transforms are dispatched from [`crate::codemod::Rule`] variants
//! and operate on Go source code via tree-sitter.

pub(crate) mod imports;
