//! Restricting one operation to a subset of the tree.
//!
//! `<lang>::symbol` rewrites every matching identifier in every file it
//! is given, with no scope analysis — which makes it the operation most
//! likely to over-apply. The DSL's `in "<glob>"` clause narrows a rule to
//! the paths that should be touched, and [`ScopedOperation`] is how that
//! clause is enforced.

use std::fmt;
use std::path::{Component, Path, PathBuf};

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
            .map_err(|e| Error::Config(format!("invalid scope glob `{pattern}`: {e}")))?;
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

/// `path` as an `in "<glob>"` scope sees it: relative to the run's base
/// ([`Run::set_base`](crate::run::Run::set_base), MCP's `cwd`) or else to
/// the working directory.
///
/// Walking `.` yields `./core/lib.rs`, and the glob `core/**` means the
/// same file, so leading `./` components are not part of the match. An
/// absolute path under the base loses the base; one outside it is matched
/// whole, so a relative glob does not match it.
pub(crate) fn scope_relative(path: &Path) -> PathBuf {
    if path.is_absolute() {
        let base = crate::run::Run::current()
            .and_then(|run| run.base())
            .or_else(|| std::env::current_dir().ok());
        if let Some(rest) = base.as_deref().and_then(|b| path.strip_prefix(b).ok()) {
            return rest.to_path_buf();
        }
        return path.to_path_buf();
    }
    path.components()
        .skip_while(|part| matches!(part, Component::CurDir))
        .collect()
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
        // The path as walked still counts, so an absolute glob over absolute
        // paths matches as it always did.
        self.inner.is_file_relevant(path)
            && (self.matcher.is_match(scope_relative(path)) || self.matcher.is_match(path))
    }

    fn apply(&self, source_code: &str) -> String {
        self.inner.apply(source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        self.inner.prefilter()
    }

    fn near_miss_target(&self) -> Option<&str> {
        self.inner.near_miss_target()
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        self.inner.observed_targets(source)
    }

    fn locate(&self, source: &str) -> Option<Vec<crate::find::Located>> {
        self.inner.locate(source)
    }

    fn conflicts(&self, before: &str, after: &str) -> Vec<String> {
        self.inner.conflicts(before, after)
    }

    fn creates(&self, path: &std::path::Path, source: &str) -> Vec<(std::path::PathBuf, String)> {
        self.inner.creates(path, source)
    }

    fn why_unchanged(&self, source: &str) -> Option<String> {
        self.inner.why_unchanged(source)
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
    fn a_leading_dot_slash_does_not_defeat_the_glob() {
        // `colab refactor -C repo --script s .` hands the walker `./…` paths.
        let op = scoped("crates/colab-core/**");
        assert!(op.is_file_relevant(Path::new("./crates/colab-core/src/lib.rs")));
        assert!(!op.is_file_relevant(Path::new("./crates/colab-cli/src/lib.rs")));
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
    fn an_absolute_path_is_matched_relative_to_the_runs_base() {
        let op = scoped("core/**");
        let base = std::env::temp_dir().join("colab-scope-base");
        let inside = base.join("core/lib.rs");
        let run = crate::run::Run::new();
        run.set_base(Some(base.clone()));
        run.enter(|| {
            assert!(op.is_file_relevant(&inside));
            assert!(!op.is_file_relevant(&base.join("other/lib.rs")));
        });
        // Without a base, the working directory is the base.
        let cwd = std::env::current_dir().unwrap();
        assert!(op.is_file_relevant(&cwd.join("core/lib.rs")));
        assert!(!op.is_file_relevant(&inside));
        // An absolute glob still matches the path as walked.
        let absolute = scoped(&format!("{}/**", base.display()));
        assert!(absolute.is_file_relevant(&inside));
    }

    #[test]
    fn an_invalid_glob_is_rejected_at_compile_time() {
        let err = ScopedOperation::new(Box::new(RustRename), "src/[").unwrap_err();
        assert!(err.to_string().contains("invalid scope glob"), "{err}");
    }
}
