//! Restricting one operation to a subset of the tree.
//!
//! `<lang>::symbol` rewrites every matching identifier in every file it
//! is given, with no scope analysis — which makes it the operation most
//! likely to over-apply. The DSL's `in "<glob>"` clause narrows a rule to
//! the paths that should be touched, and [`ScopedOperation`] is how that
//! clause is enforced.

use std::fmt;
use std::path::Path;

use globset::{Glob, GlobBuilder, GlobMatcher};

use crate::backend::Operation;
use crate::error::{Error, Result};

/// Wraps an [`Operation`] so it applies only to paths matching a glob.
///
/// # Enforcement
///
/// The scope lives entirely in [`is_file_relevant`](Operation::is_file_relevant):
/// [`apply`](Operation::apply) takes only `&str` and so *cannot* enforce
/// it. That is safe because the walker drives rules through
/// [`CodeTransformer::apply_at`](crate::CodeTransformer::apply_at), which
/// consults `is_file_relevant` per rule before applying it. Calling
/// `apply` directly bypasses the scope; don't.
pub struct ScopedOperation {
    inner: Box<dyn Operation>,
    pattern: String,
    matcher: GlobMatcher,
}

impl ScopedOperation {
    /// Restrict `inner` to paths matching `pattern`.
    ///
    /// `literal_separator` is on, so `*` stops at a path separator and
    /// `**` is what crosses directories — the behaviour `src/**/*.rs`
    /// implies to anyone who has written a gitignore.
    pub fn new(inner: Box<dyn Operation>, pattern: &str) -> Result<Self> {
        let glob: Glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| {
                Error::Config(format!("invalid scope glob `{}`: {}", pattern, e))
            })?;
        Ok(Self {
            inner,
            pattern: pattern.to_string(),
            matcher: glob.compile_matcher(),
        })
    }

    /// The glob this operation is restricted to.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

impl fmt::Debug for ScopedOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopedOperation")
            .field("pattern", &self.pattern)
            .field("inner", &self.inner)
            .finish()
    }
}

impl fmt::Display for ScopedOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} in \"{}\"", self.inner, self.pattern)
    }
}

impl Operation for ScopedOperation {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.inner.is_file_relevant(path) && self.matcher.is_match(path)
    }

    fn apply(&self, source_code: &str) -> String {
        self.inner.apply(source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        self.inner.prefilter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct RustRename;

    impl fmt::Display for RustRename {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "rust::symbol \"A\" -> \"B\"")
        }
    }

    impl Operation for RustRename {
        fn is_file_relevant(&self, path: &Path) -> bool {
            path.extension().and_then(|s| s.to_str()) == Some("rs")
        }

        fn apply(&self, source_code: &str) -> String {
            source_code.replace('A', "B")
        }

        fn prefilter(&self) -> Option<&str> {
            Some("A")
        }
    }

    fn scoped(pattern: &str) -> ScopedOperation {
        ScopedOperation::new(Box::new(RustRename), pattern).unwrap()
    }

    #[test]
    fn narrows_relevance_to_the_glob() {
        let op = scoped("crates/colab-core/**");
        assert!(op.is_file_relevant(Path::new("crates/colab-core/src/lib.rs")));
        assert!(!op.is_file_relevant(Path::new("crates/colab-cli/src/lib.rs")));
    }

    #[test]
    fn still_defers_to_the_inner_relevance_check() {
        // Inside the glob, but not a Rust file.
        let op = scoped("crates/**");
        assert!(!op.is_file_relevant(Path::new("crates/colab-core/README.md")));
    }

    #[test]
    fn single_star_does_not_cross_directories() {
        let op = scoped("src/*.rs");
        assert!(op.is_file_relevant(Path::new("src/lib.rs")));
        assert!(!op.is_file_relevant(Path::new("src/nested/lib.rs")));
    }

    #[test]
    fn delegates_apply_and_prefilter_untouched() {
        let op = scoped("**/*.rs");
        assert_eq!(op.apply("A A"), "B B");
        assert_eq!(op.prefilter(), Some("A"));
    }

    #[test]
    fn display_shows_the_scope() {
        assert_eq!(
            scoped("src/**").to_string(),
            "rust::symbol \"A\" -> \"B\" in \"src/**\""
        );
    }

    #[test]
    fn an_invalid_glob_is_rejected_at_compile_time() {
        let err = ScopedOperation::new(Box::new(RustRename), "src/[").unwrap_err();
        assert!(err.to_string().contains("invalid scope glob"), "{err}");
    }
}
