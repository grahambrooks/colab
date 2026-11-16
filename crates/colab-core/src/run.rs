//! The state one run of a script shares between its rules: the semantic
//! [`Oracle`], the facts its `scan` phases collected, and the first
//! question either could not answer.
//!
//! [`Operation::apply`](crate::Operation::apply) is string in, string
//! out, so a rule cannot be handed this state; it reads the *current*
//! run instead. A run is current on a thread inside [`Run::enter`], and
//! the walker enters the caller's run on every worker thread it uses, so
//! two runs in one process (an MCP or LSP server serving overlapping
//! requests) never see each other's oracle, facts or failures. A thread
//! that never entered a run gets a detached one of its own: nothing here
//! is process-wide.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use crate::facts::Facts;
use crate::semantic::Oracle;

/// One run's shared state. Create it with [`Run::new`], make it current
/// with [`Run::enter`].
#[derive(Default)]
pub struct Run {
    oracle: RwLock<Option<Arc<dyn Oracle>>>,
    facts: RwLock<Option<Facts>>,
    semantic_failure: Mutex<Option<String>>,
    facts_failure: Mutex<Option<String>>,
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<Run>>> = const { RefCell::new(None) };
    static DETACHED: Arc<Run> = Arc::new(Run::default());
}

impl Run {
    /// A run with no oracle and no facts.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Run `f` with this run current on this thread. The previous run is
    /// restored when `f` returns or unwinds.
    pub fn enter<T>(self: &Arc<Self>, f: impl FnOnce() -> T) -> T {
        struct Restore(Option<Arc<Run>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|c| *c.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(CURRENT.with(|c| c.replace(Some(Arc::clone(self)))));
        f()
    }

    /// The run current on this thread, if one was entered.
    pub fn current() -> Option<Arc<Self>> {
        CURRENT.with(|c| c.borrow().clone())
    }

    /// The current run, or this thread's detached one.
    pub(crate) fn here() -> Arc<Self> {
        Self::current().unwrap_or_else(|| DETACHED.with(Arc::clone))
    }

    pub(crate) fn oracle(&self) -> Option<Arc<dyn Oracle>> {
        self.oracle
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn set_oracle(&self, oracle: Option<Arc<dyn Oracle>>) {
        *self.oracle.write().unwrap_or_else(PoisonError::into_inner) = oracle;
    }

    pub(crate) fn with_facts<T>(&self, f: impl FnOnce(Option<&Facts>) -> T) -> T {
        f(self
            .facts
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref())
    }

    pub(crate) fn set_facts(&self, facts: Option<Facts>) {
        *self.facts.write().unwrap_or_else(PoisonError::into_inner) = facts;
    }

    pub(crate) fn semantic_failure(&self) -> &Mutex<Option<String>> {
        &self.semantic_failure
    }

    pub(crate) fn facts_failure(&self) -> &Mutex<Option<String>> {
        &self.facts_failure
    }

    /// The first `type` / `resolves` or `in` question this run could not
    /// answer, if any, clearing it. A run that has one must fail before
    /// it writes anything.
    pub fn take_failure(&self) -> Option<String> {
        let take = |slot: &Mutex<Option<String>>| {
            slot.lock().unwrap_or_else(PoisonError::into_inner).take()
        };
        let semantic = take(&self.semantic_failure);
        let facts = take(&self.facts_failure);
        semantic.or(facts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts;

    #[test]
    fn runs_do_not_share_facts_or_failures() {
        let (a, b) = (Run::new(), Run::new());
        a.enter(|| {
            let mut found = Facts::default();
            found.add("api", "F", "Get");
            facts::install(found);
        });
        b.enter(|| assert!(!facts::contains("api", "F", "Get")));
        assert!(b.take_failure().unwrap().contains("scan phases"));
        assert!(a.take_failure().is_none());
        a.enter(|| assert!(facts::contains("api", "F", "Get")));
    }

    #[test]
    fn a_run_entered_on_another_thread_is_the_same_run() {
        let run = Run::new();
        let seen = std::thread::scope(|s| {
            s.spawn(|| run.enter(|| Run::current().map(|r| Arc::ptr_eq(&r, &run))))
                .join()
                .unwrap()
        });
        assert_eq!(seen, Some(true));
        assert!(Run::current().is_none());
    }

    #[test]
    fn enter_restores_the_previous_run_when_f_panics() {
        let (outer, inner) = (Run::new(), Run::new());
        outer.enter(|| {
            let unwound = std::panic::catch_unwind(|| inner.enter(|| panic!("a rule panicked")));
            assert!(unwound.is_err());
            assert!(Arc::ptr_eq(&Run::current().unwrap(), &outer));
        });
    }
}
