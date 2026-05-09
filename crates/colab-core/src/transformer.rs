//! The [`CodeTransformer`] contract used by the walker.
//!
//! This trait is the only thing [`crate::walker`] knows about. Each
//! language backend or compiled refactoring implements it.

use std::path::Path;

/// A side-effect-free transformation that can be applied to source code.
///
/// Implementors decide which files they apply to via
/// [`is_file_relevant`](CodeTransformer::is_file_relevant) and produce the
/// rewritten contents via [`apply`](CodeTransformer::apply).
pub trait CodeTransformer {
    /// Returns `true` if this transformer wants to operate on `path`.
    fn is_file_relevant(&self, path: &Path) -> bool;

    /// Apply the transformation to `source_code`, returning the new contents.
    ///
    /// Implementations must return `source_code` unchanged when there is
    /// nothing to rewrite — the walker uses equality with the input to
    /// skip writes.
    fn apply(&self, source_code: &str) -> String;
}
