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
}

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
                })
                .collect(),
            ..Default::default()
        }
    }

    /// Fold one processed file into the report.
    pub fn record(&mut self, change: &FileChange) {
        self.files_scanned += 1;
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
    }

    /// Record a file that could not be read or decoded.
    pub fn skip(&mut self, path: impl Into<PathBuf>, reason: impl Into<String>) {
        self.skipped.push(SkippedFile {
            path: path.into(),
            reason: reason.into(),
        });
    }

    /// Rules that never changed a file. Empty when every rule did something.
    pub fn dead_rules(&self) -> impl Iterator<Item = &RuleStat> {
        self.rules.iter().filter(|r| r.files_matched == 0)
    }
}

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
        assert_eq!(report.dead_rules().map(|r| r.index).collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn out_of_range_rule_index_is_ignored() {
        let mut report = RunReport::with_rules(["only"]);
        report.record(&change("x.go", "a", "b", vec![0, 7]));
        assert_eq!(report.rules[0].files_matched, 1);
    }
}
