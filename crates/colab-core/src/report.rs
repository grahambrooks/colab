//! Aggregate result of one codemod run.
//!
//! [`RunReport`] is the single value both output surfaces serialize:
//! `colab refactor`'s reporters and the `colab-mcp` tool responses. Keeping
//! one type means the CLI and the MCP server cannot drift.
//!
//! The three file counters are deliberately distinct so an empty result is
//! self-explaining:
//!
//! | counters | meaning |
//! | --- | --- |
//! | `files_visited == 0` | the path or the include/exclude globs matched nothing |
//! | `files_scanned == 0` | files exist, but none belong to a language the script targets |
//! | `files_changed == 0` | files were parsed, but no rule matched — see [`RuleStat`] |
//!
//! Without that split, all three cases produce the same "nothing happened"
//! output and a caller cannot tell a mistyped path from a mistyped rule.

use std::path::PathBuf;
use std::time::Duration;

use crate::walker::FileChange;

/// Per-rule attribution for one run.
///
/// `files_matched == 0` means the rule is dead: it compiled, it ran, and it
/// never changed a byte. That is nearly always a bug in the script, so every
/// output format surfaces it.
#[derive(Debug, Clone)]
pub struct RuleStat {
    /// Zero-based position of the rule in the compiled script.
    pub index: usize,
    /// `Display` form of the rule, e.g. `go::import "a" -> "b"`.
    pub rule: String,
    /// How many files this rule actually changed.
    pub files_matched: u64,
    /// For a dead rule: the values its module saw that most resemble its
    /// target, filled in by [`crate::near_miss::probe`]. Empty otherwise.
    pub near_misses: Vec<NearMiss>,
    /// For a dead rule that did match, but left its matches alone: why,
    /// from [`Operation::why_unchanged`](crate::Operation::why_unchanged).
    pub unchanged: Option<Unchanged>,
    /// The rule's `message "…"`, shown with what it found.
    pub message: Option<String>,
    /// The rule's `severity`, `Error` unless it says otherwise.
    pub severity: Severity,
}

/// How much a rule's finding matters. `--check` fails only on `Error` and
/// `Warning`; an `Info` rule reports without failing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    #[default]
    Error,
}

impl Severity {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "error" => Some(Self::Error),
            "warning" => Some(Self::Warning),
            "info" => Some(Self::Info),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

/// A dead rule's own account of why it changed nothing where it matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unchanged {
    pub message: String,
    /// How many probed files it gave this reason for.
    pub files: u64,
}

/// A value a dead rule's module saw that resembles what it was looking
/// for, e.g. the import `a/b/v2` when the rule targets `a/b`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearMiss {
    pub value: String,
    /// How many probed files contained it.
    pub files: u64,
}

/// A rule's warning about the change it made to one file, from
/// [`Operation::conflicts`](crate::Operation::conflicts). A run that
/// writes refuses to write anything while any are present, unless told
/// to allow them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub path: PathBuf,
    /// Zero-based index of the rule, as in [`RuleStat::index`]; `None`
    /// for a conflict found in the file as a whole, such as an error the
    /// language server sees after the change (`--verify-lsp`).
    pub rule: Option<usize>,
    pub message: String,
}

/// A file the walker could not process. Recorded rather than fatal so one
/// stray non-UTF-8 blob cannot abort a whole-repo run.
#[derive(Debug, Clone)]
pub struct SkippedFile {
    pub path: PathBuf,
    pub reason: String,
}

/// Aggregate stats plus per-rule attribution for one `refactor` invocation.
#[derive(Debug, Default, Clone)]
pub struct RunReport {
    /// Files the walker yielded before the relevance check.
    pub files_visited: u64,
    /// Files that passed [`CodeTransformer::is_file_relevant`] and were read.
    ///
    /// [`CodeTransformer::is_file_relevant`]: crate::CodeTransformer::is_file_relevant
    pub files_scanned: u64,
    /// Files whose contents actually changed.
    pub files_changed: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub elapsed: Duration,
    /// One entry per rule in the compiled script, in source order.
    pub rules: Vec<RuleStat>,
    pub skipped: Vec<SkippedFile>,
    /// The first [`SCANNED_SAMPLE_LIMIT`] scanned paths, kept so a rule
    /// that matched nothing can be explained afterwards without a second
    /// walk (see [`crate::near_miss::probe`]).
    pub scanned_sample: Vec<PathBuf>,
    /// File operations the run made (or would make), in order — moves,
    /// creates, deletes. Empty for ordinary codemods.
    pub file_ops: Vec<crate::fileops::FileOp>,
    /// Run-specific warnings beyond the generic advisories, e.g. a moved
    /// file whose history git may not follow.
    pub warnings: Vec<String>,
    /// Conflicts the rules found in their own changes, in file order.
    pub conflicts: Vec<Conflict>,
    /// Which lines of each changed file each rule touched, when a CI
    /// format asked for it (see [`attribute`]). Empty otherwise, and
    /// [`crate::render::findings`] then credits every rule that changed
    /// the file.
    pub spans: std::collections::BTreeMap<PathBuf, Vec<RuleSpan>>,
    /// The branches `refactor --split-pr` made, one per rule that changed
    /// something on its own.
    pub branches: Vec<Branch>,
    /// Which rules changed each file, for the files in `scanned_sample`:
    /// the probe for skipped matches leaves a rule's own files out.
    pub fired: std::collections::BTreeMap<PathBuf, Vec<usize>>,
}

/// One branch made by `refactor --split-pr`: the base commit plus one
/// rule's changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// Zero-based index of the rule, as in [`RuleStat::index`].
    pub rule: usize,
    pub name: String,
    pub commit: String,
    pub files: usize,
}

/// Lines of the original file one rule changes (0-based, end exclusive;
/// empty for a pure insertion before `lines.start`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSpan {
    pub rule: usize,
    pub lines: std::ops::Range<usize>,
}

/// Record, for each changed file, the lines each rule that fired touches,
/// by applying that rule alone to the file as it was. A rule that only
/// changes what an earlier rule wrote touches nothing on its own; the
/// blocks nobody claims are then credited to every rule that fired.
/// Costs one apply per rule per changed file, so only the CI formats ask.
pub fn attribute(
    rules: &[Box<dyn crate::Operation>],
    changes: &[crate::walker::FileChange],
    report: &mut RunReport,
) {
    for change in changes.iter().filter(|c| c.changed()) {
        let mut spans = Vec::new();
        for &index in &change.rules_fired {
            let Some(rule) = rules.get(index) else {
                continue;
            };
            let alone = rule.apply(&change.before);
            let diff = similar::TextDiff::from_lines(change.before.as_str(), alone.as_str());
            for group in diff.grouped_ops(0) {
                if let (Some(first), Some(last)) = (group.first(), group.last()) {
                    spans.push(RuleSpan {
                        rule: index,
                        lines: first.old_range().start..last.old_range().end,
                    });
                }
            }
        }
        report.spans.insert(change.path.clone(), spans);
    }
}

/// How many scanned paths a [`RunReport`] remembers for near-miss probing.
pub const SCANNED_SAMPLE_LIMIT: usize = 10_000;

impl RunReport {
    /// Seed the per-rule table from a compiled script's rule descriptions.
    /// Call once before the walk so dead rules are present with a zero count
    /// rather than missing entirely.
    pub fn with_rules<I, S>(rules: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            rules: rules
                .into_iter()
                .enumerate()
                .map(|(index, rule)| RuleStat {
                    index,
                    rule: rule.into(),
                    files_matched: 0,
                    near_misses: Vec::new(),
                    unchanged: None,
                    message: None,
                    severity: Severity::Error,
                })
                .collect(),
            ..Default::default()
        }
    }

    /// Fold one processed file into the report.
    pub fn record(&mut self, change: &FileChange) {
        self.files_scanned += 1;
        if self.scanned_sample.len() < SCANNED_SAMPLE_LIMIT {
            self.scanned_sample.push(change.path.clone());
            if !change.rules_fired.is_empty() {
                self.fired
                    .entry(change.path.clone())
                    .or_default()
                    .extend(change.rules_fired.iter().copied());
            }
        }
        self.bytes_before += change.before.len() as u64;
        self.bytes_after += change.after.len() as u64;
        if change.changed() {
            self.files_changed += 1;
        }
        for &index in &change.rules_fired {
            if let Some(stat) = self.rules.get_mut(index) {
                stat.files_matched += 1;
            }
        }
        self.conflicts
            .extend(change.conflicts.iter().map(|c| Conflict {
                path: change.path.clone(),
                rule: Some(c.rule),
                message: c.message.clone(),
            }));
    }

    /// Fold a completed walk's outcome into the report.
    ///
    /// Every caller previously assigned `files_visited` and `skipped`
    /// field-by-field, in four places, each having to remember that
    /// multiple targets *accumulate* visits while a per-rule run must not
    /// double-count them. Doing it here means that decision is made once.
    pub fn record_walk(&mut self, outcome: crate::walker::WalkOutcome) {
        self.files_visited += outcome.files_visited;
        self.skipped.extend(outcome.skipped);
    }

    /// Record a file that could not be read or decoded.
    pub fn skip(&mut self, path: impl Into<PathBuf>, reason: impl Into<String>) {
        self.skipped.push(SkippedFile {
            path: path.into(),
            reason: reason.into(),
        });
    }

    /// `true` when a rule that may fail `--check` (severity above `info`)
    /// changed a file.
    pub fn check_fails(&self) -> bool {
        self.rules
            .iter()
            .any(|r| r.files_matched > 0 && r.severity > Severity::Info)
    }

    /// Rules that never changed a file. Empty when every rule did something.
    pub fn dead_rules(&self) -> impl Iterator<Item = &RuleStat> {
        // A tidy step that changed nothing found the code already tidy.
        self.rules
            .iter()
            .filter(|r| r.files_matched == 0 && !r.rule.starts_with(TIDY_RULE))
    }
}

/// How a `--tidy` step names itself in the report: `tidy via gofmt`.
pub const TIDY_RULE: &str = "tidy via ";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn change(path: &str, before: &str, after: &str, fired: Vec<usize>) -> FileChange {
        FileChange {
            path: Path::new(path).to_path_buf(),
            before: before.to_string(),
            after: after.to_string(),
            rules_fired: fired,
            conflicts: Vec::new(),
        }
    }

    #[test]
    fn seeds_every_rule_with_a_zero_count() {
        let report = RunReport::with_rules(["a", "b"]);
        assert_eq!(report.rules.len(), 2);
        assert_eq!(report.rules[1].index, 1);
        assert!(report.rules.iter().all(|r| r.files_matched == 0));
        assert_eq!(report.dead_rules().count(), 2);
    }

    #[test]
    fn record_tallies_per_rule_and_aggregate() {
        let mut report = RunReport::with_rules(["a", "b"]);
        report.record(&change("x.go", "one", "1", vec![0]));
        report.record(&change("y.go", "two", "two", vec![]));
        report.record(&change("z.go", "three", "33333", vec![0]));

        assert_eq!(report.files_scanned, 3);
        assert_eq!(report.files_changed, 2);
        assert_eq!(report.rules[0].files_matched, 2);
        assert_eq!(report.rules[1].files_matched, 0);
        assert_eq!(
            report.dead_rules().map(|r| r.index).collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn out_of_range_rule_index_is_ignored() {
        let mut report = RunReport::with_rules(["only"]);
        report.record(&change("x.go", "a", "b", vec![0, 7]));
        assert_eq!(report.rules[0].files_matched, 1);
    }
}
