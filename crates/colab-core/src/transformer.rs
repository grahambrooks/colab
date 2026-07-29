//! The [`CodeTransformer`] contract used by the walker.
//!
//! This trait is the only thing [`crate::walker`] knows about. Each
//! language backend or compiled refactoring implements it.

use std::path::Path;

/// Result of a path-aware apply: the rewritten source plus which rules
/// actually changed something.
///
/// `rules_fired` holds zero-based indices into the transformer's own rule
/// list, in the order the rules ran. A composite transformer that has no
/// notion of individual rules leaves it empty.
#[derive(Debug, Clone, Default)]
pub struct ApplyOutcome {
    pub output: String,
    pub rules_fired: Vec<usize>,
}

impl ApplyOutcome {
    /// An outcome with no rule attribution — the shape produced by the
    /// default [`CodeTransformer::apply_at`].
    pub fn untraced(output: String) -> Self {
        Self {
            output,
            rules_fired: Vec::new(),
        }
    }
}

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

    /// Path-aware apply. This is what the walker calls.
    ///
    /// Knowing the path lets a composite transformer skip rules that are
    /// irrelevant to this file instead of relying on each rule to be a no-op,
    /// and lets it report which rules fired. The default implementation
    /// delegates to [`apply`](CodeTransformer::apply) and reports no
    /// attribution, so single-rule implementors need not override it.
    fn apply_at(&self, _path: &Path, source_code: &str) -> ApplyOutcome {
        ApplyOutcome::untraced(self.apply(source_code))
    }
}
