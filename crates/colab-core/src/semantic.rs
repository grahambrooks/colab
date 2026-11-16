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
    /// `implements "io.Reader"`: the binding (a type) has a supertype —
    /// an interface it implements, a trait, a base class — that resolves
    /// to this, as the server's type hierarchy reports it, at any depth.
    Implements(String),
    /// `calls "pkg.Func"`: the binding (a function) calls a function that
    /// resolves to this, as the server's call hierarchy reports it.
    Calls(String),
    /// `called_by "pkg.Func"`: the binding (a function) is called from a
    /// function that resolves to this.
    CalledBy(String),
}

impl std::fmt::Display for Query {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Query::Type(t) => write!(f, "type \"{t}\""),
            Query::Resolves(r) => write!(f, "resolves \"{r}\""),
            Query::Implements(t) => write!(f, "implements \"{t}\""),
            Query::Calls(t) => write!(f, "calls \"{t}\""),
            Query::CalledBy(t) => write!(f, "called_by \"{t}\""),
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

    /// The same `query` about several bindings of one file, answered in
    /// `spans` order. The default asks each in turn; an oracle that can
    /// send them together (a language server) should.
    fn ask_all(
        &self,
        path: &Path,
        source: &str,
        spans: &[Range<usize>],
        query: &Query,
    ) -> Vec<Result<bool, String>> {
        spans
            .iter()
            .map(|span| self.ask(path, source, span.clone(), query))
            .collect()
    }
}

/// What an answer is about: the file, its text (by hash), the binding and
/// the question. The text is part of the key, so a rule that changed the
/// file asks again.
pub(crate) type AnswerKey = (PathBuf, u64, Range<usize>, String);

fn answer_key(path: &Path, source: &str, span: &Range<usize>, query: &Query) -> AnswerKey {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    (
        path.to_path_buf(),
        hasher.finish(),
        span.clone(),
        query.to_string(),
    )
}

fn record_failure(run: &Run, why: String) {
    run.semantic_failure()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert(why);
}

thread_local! {
    static PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Answer the current [`Run`]'s queries with `oracle` until [`uninstall`].
pub fn install(oracle: Arc<dyn Oracle>) {
    let run = Run::here();
    run.set_oracle(Some(oracle));
    // Answers from another oracle are not this one's.
    run.forget_answers();
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
    if let Some(path) = &path
        && let Some(known) = run.answer(&answer_key(path, source, &span, query))
    {
        return known;
    }
    let answer = match (oracle, path) {
        (Some(oracle), Some(path)) => {
            let answer = oracle.ask(&path, source, span.clone(), query);
            if let Ok(holds) = answer {
                run.remember(answer_key(&path, source, &span, query), holds);
            }
            answer
        }
        (None, _) => Err(format!(
            "`{query}` needs a language server to answer it, and none is available here; \
             run the rule with the colab CLI"
        )),
        (Some(_), None) => Err(format!("`{query}` was asked without a file to ask about")),
    };
    match answer {
        Ok(holds) => holds,
        Err(why) => {
            record_failure(&run, why);
            false
        }
    }
}

/// Ask `query` about every binding in `spans` of the current file at
/// once ([`Oracle::ask_all`]), keeping the answers for [`ask`]. The
/// pattern engine calls it with every match its cheaper checks kept, so a
/// file's questions cost one batch, not one round trip each. Without an
/// oracle or a file it does nothing: [`ask`] then records why.
pub fn prefetch(source: &str, spans: &[Range<usize>], query: &Query) {
    let run = Run::here();
    let (Some(oracle), Some(path)) = (run.oracle(), PATH.with(|p| p.borrow().clone())) else {
        return;
    };
    let unknown: Vec<Range<usize>> = spans
        .iter()
        .filter(|span| {
            run.answer(&answer_key(&path, source, span, query))
                .is_none()
        })
        .cloned()
        .collect();
    if unknown.is_empty() {
        return;
    }
    let answers = oracle.ask_all(&path, source, &unknown, query);
    for (span, answer) in unknown.iter().zip(answers) {
        match answer {
            Ok(holds) => run.remember(answer_key(&path, source, span, query), holds),
            Err(why) => {
                // Kept as `false` so the match is not asked again, and the
                // run fails with the reason.
                record_failure(&run, why);
                run.remember(answer_key(&path, source, span, query), false);
            }
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

    /// Counts what it is asked: single questions, and batches.
    #[derive(Default)]
    struct Counting {
        asked: std::sync::atomic::AtomicUsize,
        batches: std::sync::atomic::AtomicUsize,
    }

    impl Oracle for Counting {
        fn ask(&self, _: &Path, _: &str, span: Range<usize>, _: &Query) -> Result<bool, String> {
            self.asked
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(span.start == 0)
        }
        fn ask_all(
            &self,
            _: &Path,
            _: &str,
            spans: &[Range<usize>],
            _: &Query,
        ) -> Vec<Result<bool, String>> {
            self.batches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            spans.iter().map(|s| Ok(s.start == 0)).collect()
        }
    }

    #[test]
    fn a_prefetched_batch_answers_later_questions_without_asking_again() {
        use std::sync::atomic::Ordering::Relaxed;
        let oracle = Arc::new(Counting::default());
        let run = Run::new();
        let query = Query::Type("T".into());
        run.enter(|| {
            install(oracle.clone());
            at_path(Path::new("a.go"), || {
                prefetch("a b c", &[0..1, 2..3, 4..5], &query);
                assert!(ask("a b c", 0..1, &query));
                assert!(!ask("a b c", 2..3, &query));
                assert!(!ask("a b c", 4..5, &query));
                // A second batch over the same text asks nothing new.
                prefetch("a b c", &[0..1, 2..3], &query);
                // Another text is another question.
                assert!(!ask("a b d", 2..3, &query));
            });
        });
        assert_eq!(oracle.batches.load(Relaxed), 1);
        assert_eq!(oracle.asked.load(Relaxed), 1, "only the changed text");
        assert!(run.take_failure().is_none());
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
