//! Import clauses on a rule: `requires import "fmt"` and
//! `drops import "github.com/pkg/errors" when unused`.
//!
//! A template that writes `fmt.Errorf(…)` needs `fmt` imported, but only
//! in the files it actually rewrote. [`WithFollowups`] runs the rule and,
//! only when it changed the file, runs each follow-up on the result. The
//! whole thing is one [`Operation`], so the walker's per-file sequencing
//! and per-rule attribution are untouched.

use std::fmt;
use std::path::Path;

use crate::backend::Operation;
use crate::find::Located;

/// One operation that runs after the rule, only on files the rule changed.
#[derive(Debug)]
pub struct Followup {
    /// The clause as written, e.g. `requires import "fmt"`.
    pub label: String,
    pub op: Box<dyn Operation>,
}

/// A rule plus the follow-ups its import clauses lowered to.
#[derive(Debug)]
pub struct WithFollowups {
    inner: Box<dyn Operation>,
    followups: Vec<Followup>,
}

impl WithFollowups {
    pub fn new(inner: Box<dyn Operation>, followups: Vec<Followup>) -> Self {
        Self { inner, followups }
    }
}

impl fmt::Display for WithFollowups {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)?;
        for followup in &self.followups {
            write!(f, " {}", followup.label)?;
        }
        Ok(())
    }
}

impl Operation for WithFollowups {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.inner.is_file_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        let changed = self.inner.apply(source_code);
        if changed == source_code {
            // Nothing rewritten, so nothing to import or drop. This is also
            // what makes the rule idempotent: a second pass rewrites
            // nothing and so adds nothing.
            return changed;
        }
        self.followups
            .iter()
            .fold(changed, |source, followup| followup.op.apply(&source))
    }

    /// The rule's own prefilter: without it the rule is a no-op, and so are
    /// the follow-ups.
    fn prefilter(&self) -> Option<&str> {
        self.inner.prefilter()
    }

    fn near_miss_target(&self) -> Option<&str> {
        self.inner.near_miss_target()
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        self.inner.observed_targets(source)
    }

    fn locate(&self, source: &str) -> Option<Vec<Located>> {
        self.inner.locate(source)
    }

    fn conflicts(&self, before: &str, after: &str) -> Vec<String> {
        self.inner.conflicts(before, after)
    }

    fn why_unchanged(&self, source: &str) -> Option<String> {
        self.inner.why_unchanged(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Replace(&'static str, &'static str);

    impl fmt::Display for Replace {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{} -> {}", self.0, self.1)
        }
    }

    impl Operation for Replace {
        fn is_file_relevant(&self, _: &Path) -> bool {
            true
        }
        fn apply(&self, source: &str) -> String {
            source.replace(self.0, self.1)
        }
        fn prefilter(&self) -> Option<&str> {
            Some(self.0)
        }
    }

    #[derive(Debug)]
    struct Prepend(&'static str);

    impl fmt::Display for Prepend {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "prepend {}", self.0)
        }
    }

    impl Operation for Prepend {
        fn is_file_relevant(&self, _: &Path) -> bool {
            true
        }
        fn apply(&self, source: &str) -> String {
            if source.starts_with(self.0) {
                source.to_string()
            } else {
                format!("{}{source}", self.0)
            }
        }
    }

    fn op() -> WithFollowups {
        WithFollowups::new(
            Box::new(Replace("old", "new")),
            vec![Followup {
                label: "requires import \"x\"".into(),
                op: Box::new(Prepend("import x\n")),
            }],
        )
    }

    #[test]
    fn follow_ups_run_only_when_the_rule_changed_the_file() {
        assert_eq!(op().apply("old()"), "import x\nnew()");
        assert_eq!(op().apply("other()"), "other()");
    }

    #[test]
    fn is_idempotent_and_keeps_the_rule_prefilter() {
        let once = op().apply("old()");
        assert_eq!(op().apply(&once), once);
        assert_eq!(op().prefilter(), Some("old"));
        assert_eq!(op().to_string(), "old -> new requires import \"x\"");
    }
}
