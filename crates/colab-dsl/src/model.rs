//! Runtime intermediate representation of a compiled codemod script.
//!
//! [`Refactoring`] is the value [`crate::compile`] produces. It holds a
//! list of [`Operation`]s sourced from registered language backends and
//! implements [`CodeTransformer`] so the [`colab_core::walker`] can
//! apply it without knowing about any specific language.

use std::fmt;
use std::path::Path;

use colab_core::{ApplyOutcome, CodeTransformer, Operation};

/// The compiled, executable form of a codemod script.
pub struct Refactoring {
    pub name: String,
    /// Operations applied in source order. Each rule must be a no-op
    /// for files it does not care about so composition is safe across
    /// languages.
    pub rules: Vec<Box<dyn Operation>>,
}

impl Refactoring {
    /// Returns `true` if the script contains no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Number of rules in the script.
    pub fn len(&self) -> usize {
        self.rules.len()
    }
}

impl fmt::Debug for Refactoring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Refactoring")
            .field("name", &self.name)
            .field("rules", &self.rules)
            .finish()
    }
}

impl fmt::Display for Refactoring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Refactoring '{}': [", self.name)?;
        for (i, rule) in self.rules.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", rule)?;
        }
        write!(f, "]")
    }
}

impl CodeTransformer for Refactoring {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.rules.iter().any(|r| r.is_file_relevant(path))
    }

    /// Path-blind apply: every rule runs, relying on each one being a
    /// no-op for input it does not care about. Prefer
    /// [`apply_at`](CodeTransformer::apply_at), which the walker uses —
    /// it can skip irrelevant rules outright instead of parsing to
    /// discover they had nothing to do.
    fn apply(&self, source_code: &str) -> String {
        let mut current = source_code.to_string();
        for rule in &self.rules {
            current = rule.apply(&current);
        }
        current
    }

    /// Apply every rule that could possibly match this file, in source
    /// order, reporting which ones actually changed something.
    ///
    /// Two filters run before the expensive tree-sitter parse:
    ///
    /// 1. **Path gating** — a rule whose `is_file_relevant` rejects the
    ///    path is skipped, so a Go rule never parses a `.rs` file. This
    ///    turns the "irrelevant rules must be identity" contract from an
    ///    assumption into something the engine enforces, and it is what
    ///    makes `in "<glob>"` scoping work.
    /// 2. **Literal prefilter** — a rule whose target literal is absent
    ///    from the source cannot change it, so the parse is skipped.
    ///
    /// The prefilter tests `current`, not `source_code`, so a rule whose
    /// target was introduced by an earlier rule still fires.
    fn apply_at(&self, path: &Path, source_code: &str) -> ApplyOutcome {
        let mut current = source_code.to_string();
        let mut rules_fired = Vec::new();

        for (index, rule) in self.rules.iter().enumerate() {
            if !rule.is_file_relevant(path) {
                continue;
            }
            if let Some(literal) = rule.prefilter()
                && !current.contains(literal)
            {
                continue;
            }
            let next = rule.apply(&current);
            if next != current {
                rules_fired.push(index);
                current = next;
            }
        }

        ApplyOutcome {
            output: current,
            rules_fired,
        }
    }
}

/// A single-rule view over one [`Operation`], implementing
/// [`CodeTransformer`] so the walker can apply it in isolation.
///
/// Used by `colab refactor --verify` and `--commit-per-rule` to
/// run rules one at a time so failures can be attributed to (and
/// reverted at) the rule level.
pub struct SingleRule<'a> {
    pub op: &'a dyn Operation,
}

impl<'a> SingleRule<'a> {
    pub fn new(op: &'a dyn Operation) -> Self {
        Self { op }
    }
}

impl CodeTransformer for SingleRule<'_> {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.op.is_file_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        self.op.apply(source_code)
    }

    fn apply_at(&self, path: &Path, source_code: &str) -> ApplyOutcome {
        if !self.op.is_file_relevant(path) {
            return ApplyOutcome::untraced(source_code.to_string());
        }
        if let Some(literal) = self.op.prefilter()
            && !source_code.contains(literal)
        {
            return ApplyOutcome::untraced(source_code.to_string());
        }
        let output = self.op.apply(source_code);
        // Index 0: the caller re-indexes this against the full script.
        let rules_fired = if output == source_code {
            Vec::new()
        } else {
            vec![0]
        };
        ApplyOutcome {
            output,
            rules_fired,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A test operation that counts how many times `apply` ran, so tests
    /// can assert the gate and the prefilter actually avoided the
    /// (expensive) parse rather than merely producing unchanged output.
    ///
    /// `needle: None` stands in for an `ensure` rule: it has no prefilter
    /// and acts when its target is *absent*.
    #[derive(Debug)]
    struct CountingOp {
        extension: &'static str,
        needle: Option<&'static str>,
        replacement: &'static str,
        applies: Arc<AtomicUsize>,
    }

    impl CountingOp {
        /// Returns the boxed operation plus a handle to its apply counter.
        fn build(
            extension: &'static str,
            needle: Option<&'static str>,
            replacement: &'static str,
        ) -> (Box<dyn Operation>, Arc<AtomicUsize>) {
            let applies = Arc::new(AtomicUsize::new(0));
            let op = Box::new(Self {
                extension,
                needle,
                replacement,
                applies: Arc::clone(&applies),
            });
            (op, applies)
        }
    }

    impl fmt::Display for CountingOp {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "{}::test \"{}\"",
                self.extension,
                self.needle.unwrap_or("<ensure>")
            )
        }
    }

    impl Operation for CountingOp {
        fn is_file_relevant(&self, path: &Path) -> bool {
            path.extension().and_then(|s| s.to_str()) == Some(self.extension)
        }

        fn apply(&self, source_code: &str) -> String {
            self.applies.fetch_add(1, Ordering::Relaxed);
            match self.needle {
                Some(needle) => source_code.replace(needle, self.replacement),
                None if source_code.contains(self.replacement) => source_code.to_string(),
                None => format!("{}{}", source_code, self.replacement),
            }
        }

        fn prefilter(&self) -> Option<&str> {
            self.needle
        }
    }

    fn refactoring(rules: Vec<Box<dyn Operation>>) -> Refactoring {
        Refactoring {
            name: "test".to_string(),
            rules,
        }
    }

    fn count(counter: &Arc<AtomicUsize>) -> usize {
        counter.load(Ordering::Relaxed)
    }

    #[test]
    fn irrelevant_rules_are_never_applied() {
        let (go, go_applies) = CountingOp::build("go", Some("alpha"), "beta");
        let (rs, rs_applies) = CountingOp::build("rs", Some("alpha"), "gamma");

        let refactoring = refactoring(vec![go, rs]);
        let outcome = refactoring.apply_at(Path::new("x.rs"), "alpha");

        assert_eq!(outcome.output, "gamma");
        assert_eq!(outcome.rules_fired, vec![1]);
        // The Go rule never parsed the Rust file — this is the win over
        // relying on it to be a no-op.
        assert_eq!(count(&go_applies), 0);
        assert_eq!(count(&rs_applies), 1);
    }

    #[test]
    fn prefilter_skips_the_parse_when_the_literal_is_absent() {
        let (op, applies) = CountingOp::build("go", Some("needle"), "replaced");
        let refactoring = refactoring(vec![op]);

        let outcome = refactoring.apply_at(Path::new("x.go"), "nothing here");
        assert_eq!(outcome.output, "nothing here");
        assert!(outcome.rules_fired.is_empty());
        assert_eq!(count(&applies), 0);

        let outcome = refactoring.apply_at(Path::new("x.go"), "a needle b");
        assert_eq!(outcome.output, "a replaced b");
        assert_eq!(outcome.rules_fired, vec![0]);
        assert_eq!(count(&applies), 1);
    }

    #[test]
    fn ensure_style_rule_fires_when_the_target_is_absent() {
        // The regression this guards: giving an `ensure` operation a
        // prefilter would make it a no-op in exactly the files that need
        // the insert.
        let (op, applies) = CountingOp::build("go", None, "\nimport \"ctx\"");
        let refactoring = refactoring(vec![op]);

        let outcome = refactoring.apply_at(Path::new("x.go"), "package demo");
        assert_eq!(outcome.output, "package demo\nimport \"ctx\"");
        assert_eq!(outcome.rules_fired, vec![0]);
        assert_eq!(count(&applies), 1);
    }

    #[test]
    fn prefilter_sees_text_introduced_by_an_earlier_rule() {
        let (first, _) = CountingOp::build("go", Some("one"), "two");
        let (second, _) = CountingOp::build("go", Some("two"), "three");
        let refactoring = refactoring(vec![first, second]);

        let outcome = refactoring.apply_at(Path::new("x.go"), "one");
        assert_eq!(outcome.output, "three");
        assert_eq!(outcome.rules_fired, vec![0, 1]);
    }

    #[test]
    fn rules_that_change_nothing_are_not_reported_as_fired() {
        let (hit, _) = CountingOp::build("go", Some("a"), "A");
        // Present in the source, so the prefilter passes, but `apply`
        // returns the input unchanged — still not "fired".
        let (miss, miss_applies) = CountingOp::build("go", Some("z"), "z");
        let refactoring = refactoring(vec![hit, miss]);

        let outcome = refactoring.apply_at(Path::new("x.go"), "a z");
        assert_eq!(outcome.output, "A z");
        assert_eq!(outcome.rules_fired, vec![0]);
        assert_eq!(count(&miss_applies), 1);
    }

    #[test]
    fn path_blind_apply_still_runs_every_rule() {
        // `apply` keeps its old semantics for callers that have no path.
        let (go, go_applies) = CountingOp::build("go", Some("a"), "b");
        let (rs, _) = CountingOp::build("rs", Some("b"), "c");
        let refactoring = refactoring(vec![go, rs]);

        assert_eq!(refactoring.apply("a"), "c");
        assert_eq!(count(&go_applies), 1);
    }

    #[test]
    fn single_rule_reports_index_zero_and_honours_the_gate() {
        let (op, applies) = CountingOp::build("go", Some("a"), "b");
        let single = SingleRule::new(op.as_ref());

        let outcome = single.apply_at(Path::new("x.rs"), "a");
        assert_eq!(outcome.output, "a");
        assert!(outcome.rules_fired.is_empty());
        assert_eq!(count(&applies), 0);

        let outcome = single.apply_at(Path::new("x.go"), "a");
        assert_eq!(outcome.output, "b");
        assert_eq!(outcome.rules_fired, vec![0]);
        assert_eq!(count(&applies), 1);
    }
}
