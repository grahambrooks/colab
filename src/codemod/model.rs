//! Runtime intermediate representation of a compiled codemod script.
//!
//! [`Refactoring`] is the value the [`crate::codemod::compile`] function
//! produces. It implements [`CodeTransformer`], so the [`crate::walker`]
//! can apply it to source files without knowing about specific languages.
//! Each variant of [`Rule`] encapsulates a single language-specific
//! transformation.

use std::fmt;
use std::path::Path;

use crate::codemod::go;

/// A side-effect-free transformation that can be applied to source code.
///
/// Implementors decide which files they apply to via [`is_file_relevant`]
/// and produce the rewritten contents via [`apply`].
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

/// The compiled, executable form of a codemod script.
#[derive(Debug)]
pub struct Refactoring {
    pub name: String,
    pub rule: Rule,
}

/// A single transformation rule. New language operations are added as
/// additional variants.
#[derive(Debug)]
pub enum Rule {
    /// Replace a Go import path everywhere it appears in `*.go` files.
    GoImportRename { from: String, to: String },
}

impl fmt::Display for Refactoring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Refactoring '{}': {}", self.name, self.rule)
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rule::GoImportRename { from, to } => {
                write!(f, "go::import \"{}\" -> \"{}\"", from, to)
            }
        }
    }
}

impl CodeTransformer for Refactoring {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.rule.is_file_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        self.rule.apply(source_code)
    }
}

impl CodeTransformer for Rule {
    fn is_file_relevant(&self, path: &Path) -> bool {
        match self {
            Rule::GoImportRename { .. } => {
                path.extension().and_then(|s| s.to_str()) == Some("go")
            }
        }
    }

    fn apply(&self, source_code: &str) -> String {
        match self {
            Rule::GoImportRename { from, to } => go::imports::rename(from, to, source_code),
        }
    }
}
