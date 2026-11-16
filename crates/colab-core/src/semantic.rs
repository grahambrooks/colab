//! `where $X type "…"` and `where $F resolves "…"`: constraints only a
//! language server can answer.
//!
//! colab computes nothing semantic. A rule with such a constraint asks an
//! [`Oracle`] about each match the syntactic pattern (and its cheaper
//! constraints) already found; the `colab` binary installs one that asks
//! the language's server. [`Operation::apply`](crate::Operation::apply)
//! is still string in, string out, so the oracle learns which file it is
//! asked about from [`at_path`], which [`CodeTransformer::apply_at`] and
//! [`crate::find::locate_all`] wrap around their rules.
//!
//! An answer the oracle cannot give — no server for the language, one
//! still indexing at the timeout, no oracle at all — never quietly
//! matches everything or nothing: the check fails, and the failure is
//! kept for the run to report ([`take_failure`]) before anything is
//! written.
//!
//! [`CodeTransformer::apply_at`]: crate::CodeTransformer::apply_at

use std::cell::RefCell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::run::Run;

/// What a constraint asks about a binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    /// `type "io.Reader"`: the binding's type, as the server spells it in
    /// its hover, contains this.
    Type(String),
    /// `resolves "github.com/pkg/errors.Wrap"`: the binding refers to a
    /// declaration named `Wrap` in a file whose path holds the segments
    /// `github.com`, `pkg`, `errors`, in order.
    Resolves(String),
}

impl std::fmt::Display for Query {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Query::Type(t) => write!(f, "type \"{t}\""),
            Query::Resolves(r) => write!(f, "resolves \"{r}\""),
        }
    }
}

/// Answers [`Query`]s, for the `colab` binary through a language server.
pub trait Oracle: Send + Sync {
    /// Whether the binding at `span` of `source` (the file at `path`, as
    /// the run has it, earlier rules' edits included) satisfies `query`,
    /// or why that cannot be told.
    fn ask(
        &self,
        path: &Path,
        source: &str,
        span: Range<usize>,
        query: &Query,
    ) -> Result<bool, String>;
}

thread_local! {
    static PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Answer the current [`Run`]'s queries with `oracle` until [`uninstall`].
pub fn install(oracle: Arc<dyn Oracle>) {
    let run = Run::here();
    run.set_oracle(Some(oracle));
    take_failure();
}

/// Stop answering the current run's queries; later ones fail.
pub fn uninstall() {
    Run::here().set_oracle(None);
}

/// Run `f` with queries asked about the file at `path`.
///
/// The previous path is restored when `f` returns or unwinds: rayon's
/// worker threads outlive a run, so a panicking rule must not leave the
/// next run on that thread asking about this file.
pub fn at_path<T>(path: &Path, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PATH.with(|p| *p.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(PATH.with(|p| p.replace(Some(path.to_path_buf()))));
    f()
}

/// Whether the binding at `span` of `source` satisfies `query`. `false`
/// when it cannot be told, with the reason kept for [`take_failure`].
pub fn ask(source: &str, span: Range<usize>, query: &Query) -> bool {
    let run = Run::here();
    let oracle = run.oracle();
    let path = PATH.with(|p| p.borrow().clone());
    let answer = match (oracle, path) {
        (Some(oracle), Some(path)) => oracle.ask(&path, source, span, query),
        (None, _) => Err(format!(
            "`{query}` needs a language server to answer it, and none is available here; \
             run the rule with the colab CLI"
        )),
        (Some(_), None) => Err(format!("`{query}` was asked without a file to ask about")),
    };
    match answer {
        Ok(holds) => holds,
        Err(why) => {
            let mut failure = run
                .semantic_failure()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            failure.get_or_insert(why);
            false
        }
    }
}

/// The first query the current run could not answer since the last
/// call, if any. A run that gets one must fail rather than report its
/// matches.
pub fn take_failure() -> Option<String> {
    Run::here()
        .semantic_failure()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_path_restores_the_previous_path_when_f_panics() {
        let current = || PATH.with(|p| p.borrow().clone());
        at_path(Path::new("outer.rs"), || {
            let unwound = std::panic::catch_unwind(|| {
                at_path(Path::new("inner.rs"), || panic!("a rule panicked"));
            });
            assert!(unwound.is_err());
            assert_eq!(current().as_deref(), Some(Path::new("outer.rs")));
        });
        assert_eq!(current(), None);
    }

    struct Fixed(bool);

    impl Oracle for Fixed {
        fn ask(
            &self,
            path: &Path,
            source: &str,
            span: Range<usize>,
            _: &Query,
        ) -> Result<bool, String> {
            assert_eq!(path, Path::new("a.go"));
            assert_eq!(&source[span], "r");
            Ok(self.0)
        }
    }

    #[test]
    fn answers_come_from_the_oracle_and_failures_are_kept() {
        let q = Query::Type("io.Reader".into());
        // Without an oracle: false, and the run is told why.
        uninstall();
        take_failure();
        assert!(!at_path(Path::new("a.go"), || ask("r", 0..1, &q)));
        assert!(take_failure().unwrap().contains("needs a language server"));
        install(Arc::new(Fixed(true)));
        assert!(at_path(Path::new("a.go"), || ask("r", 0..1, &q)));
        assert!(take_failure().is_none());
        uninstall();
    }
}
