//! What a script's `scan` phases collected, for `where $X in <name>`.
//!
//! ```text
//! scan go::pattern 'func $F($$$P) error { $$$B }' in "api/**" as api_funcs
//! match go::pattern '$PKG.$F($$$A)' where $F in api_funcs { … }
//! ```
//!
//! A scan walks every file before the rules of its stage run, read-only,
//! and records the text of each binding of each match under
//! `<name>.<VAR>`. Rules still edit one file each; they only *read* what
//! the scan saw in other files, as plain text: nothing resolves names.
//! A phased run ([`install`]s the facts after each stage's scans); a check
//! asked with none installed (a surface that cannot run scans) fails
//! rather than answer from an empty set ([`take_failure`]).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, RwLock};

/// Each scan's bindings: set name → metavariable (no sigil) → texts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub sets: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

impl Facts {
    /// Note that `set` exists, so a scan that matched nothing still
    /// answers (with "not in").
    pub fn declare(&mut self, set: &str) {
        self.sets.entry(set.to_string()).or_default();
    }

    pub fn add(&mut self, set: &str, var: &str, text: &str) {
        self.sets
            .entry(set.to_string())
            .or_default()
            .entry(var.to_string())
            .or_default()
            .insert(text.to_string());
    }

    /// How many distinct values each set holds, for the report.
    pub fn sizes(&self) -> Vec<(String, usize)> {
        self.sets
            .iter()
            .map(|(name, vars)| {
                (
                    name.clone(),
                    vars.values().map(BTreeSet::len).max().unwrap_or(0),
                )
            })
            .collect()
    }
}

static FACTS: RwLock<Option<Facts>> = RwLock::new(None);
static FAILURE: Mutex<Option<String>> = Mutex::new(None);

/// Answer `in` checks from `facts` until [`uninstall`].
pub fn install(facts: Facts) {
    *FACTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(facts);
}

pub fn uninstall() {
    *FACTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// Whether `text` was bound to `var` by the scan `set`. `false` when no
/// facts are installed, the reason kept for [`take_failure`].
pub fn contains(set: &str, var: &str, text: &str) -> bool {
    let facts = FACTS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match facts.as_ref() {
        Some(facts) => facts
            .sets
            .get(set)
            .and_then(|vars| vars.get(var))
            .is_some_and(|values| values.contains(text)),
        None => {
            FAILURE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get_or_insert_with(|| {
                    format!(
                        "`in {set}` needs the script's scan phases, which this run did not make; \
                         run it with `colab refactor`, `colab fix` or `colab find`"
                    )
                });
            false
        }
    }
}

/// The first `in` check that had no facts to ask, since the last call.
pub fn take_failure() -> Option<String> {
    FAILURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_read_the_installed_facts_and_fail_without_them() {
        uninstall();
        take_failure();
        assert!(!contains("api", "F", "Get"));
        assert!(take_failure().unwrap().contains("scan phases"));
        let mut facts = Facts::default();
        facts.add("api", "F", "Get");
        facts.declare("empty");
        install(facts.clone());
        assert!(contains("api", "F", "Get"));
        assert!(!contains("api", "F", "Put"));
        assert!(!contains("empty", "F", "Get"));
        assert!(take_failure().is_none());
        assert_eq!(facts.sizes(), [("api".into(), 1), ("empty".into(), 0)]);
        uninstall();
    }
}
