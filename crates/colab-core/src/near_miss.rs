//! Explain a rule that matched nothing.
//!
//! `matched no files: go::import "a/b" -> "c"` says *that* a rule is
//! dead, not *why*. Nearly always the target is slightly wrong — a
//! missing `/v2`, the wrong host, a typo — and the fix is obvious once the
//! values that *are* present are on screen. [`probe`] collects those
//! values for each dead rule from a bounded sample of the files the run
//! scanned, and stores the closest few on the rule's [`RuleStat`].
//!
//! [`RuleStat`]: crate::report::RuleStat

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::backend::Operation;
use crate::report::{NearMiss, RunReport};
use crate::suggest;

/// The most files read per run while probing. Probing re-parses, so it
/// is bounded; a target that is present at all is usually present early.
pub const PROBE_FILE_LIMIT: usize = 500;

/// The most near misses reported per rule.
pub const NEAR_MISS_LIMIT: usize = 3;

/// Fill in [`RuleStat::near_misses`](crate::report::RuleStat) for every
/// dead rule that supports it.
///
/// `rules` must be the compiled script in the same order as
/// `report.rules`. `read` loads a sampled path's current contents; a
/// `None` (unreadable, vanished) just skips the file. Only paths a dead
/// rule considers relevant are read, and at most [`PROBE_FILE_LIMIT`].
pub fn probe<R>(rules: &[Box<dyn Operation>], report: &mut RunReport, mut read: R)
where
    R: FnMut(&Path) -> Option<String>,
{
    let dead: Vec<(usize, &str)> = report
        .dead_rules()
        .filter_map(|stat| {
            let target = rules.get(stat.index)?.near_miss_target()?;
            Some((stat.index, target))
        })
        .collect();
    if dead.is_empty() {
        return;
    }

    // rule index -> observed value -> number of files it appeared in.
    let mut seen: BTreeMap<usize, BTreeMap<String, u64>> = BTreeMap::new();
    let mut files_read = 0;
    for path in &report.scanned_sample {
        if files_read >= PROBE_FILE_LIMIT {
            break;
        }
        let relevant: Vec<usize> = dead
            .iter()
            .map(|&(index, _)| index)
            .filter(|&index| rules[index].is_file_relevant(path))
            .collect();
        if relevant.is_empty() {
            continue;
        }
        let Some(source) = read(path) else {
            continue;
        };
        files_read += 1;
        for index in relevant {
            let values: BTreeSet<String> =
                rules[index].observed_targets(&source).into_iter().collect();
            let counts = seen.entry(index).or_default();
            for value in values {
                *counts.entry(value).or_default() += 1;
            }
        }
    }

    for (index, target) in dead {
        let Some(counts) = seen.remove(&index) else {
            continue;
        };
        report.rules[index].near_misses = rank(target, counts);
    }
}

/// [`probe`], reading sampled files from disk.
pub fn probe_disk(rules: &[Box<dyn Operation>], report: &mut RunReport) {
    probe(rules, report, |path| std::fs::read_to_string(path).ok());
}

/// The values in `counts` that resemble `target`, closest first, ties
/// broken by how many files they appeared in.
fn rank(target: &str, counts: BTreeMap<String, u64>) -> Vec<NearMiss> {
    let mut candidates: Vec<(usize, NearMiss)> = counts
        .into_iter()
        .filter_map(|(value, files)| {
            let distance = suggest::near_miss_distance(target, &value)?;
            Some((distance, NearMiss { value, files }))
        })
        .collect();
    candidates.sort_by_key(|(distance, miss)| (*distance, Reverse(miss.files)));
    candidates
        .into_iter()
        .take(NEAR_MISS_LIMIT)
        .map(|(_, miss)| miss)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::path::PathBuf;

    use super::*;
    use crate::walker::FileChange;

    /// A fake import rule: its "module" sees one value per line.
    #[derive(Debug)]
    struct LineImport(&'static str);

    impl fmt::Display for LineImport {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "fake::import \"{}\"", self.0)
        }
    }

    impl Operation for LineImport {
        fn is_file_relevant(&self, path: &Path) -> bool {
            path.extension().is_some_and(|e| e == "fake")
        }
        fn apply(&self, source: &str) -> String {
            source.to_string()
        }
        fn near_miss_target(&self) -> Option<&str> {
            Some(self.0)
        }
        fn observed_targets(&self, source: &str) -> Vec<String> {
            source.lines().map(str::to_string).collect()
        }
    }

    fn scanned(report: &mut RunReport, path: &str) {
        report.record(&FileChange {
            path: PathBuf::from(path),
            before: String::new(),
            after: String::new(),
            rules_fired: Vec::new(),
        });
    }

    #[test]
    fn dead_rule_gets_the_closest_observed_values() {
        let rules: Vec<Box<dyn Operation>> = vec![Box::new(LineImport("a/b"))];
        let mut report = RunReport::with_rules(rules.iter().map(|r| r.to_string()));
        scanned(&mut report, "one.fake");
        scanned(&mut report, "two.fake");
        scanned(&mut report, "skip.other");

        let mut reads = Vec::new();
        probe(&rules, &mut report, |path| {
            reads.push(path.to_path_buf());
            Some(match path.to_str().unwrap() {
                "one.fake" => "a/b/v2\nfmt\n".to_string(),
                _ => "a/b/v2\na/c\n".to_string(),
            })
        });

        assert_eq!(reads.len(), 2, "irrelevant files are not read");
        assert_eq!(
            report.rules[0].near_misses,
            vec![
                NearMiss {
                    value: "a/c".into(),
                    files: 1
                },
                NearMiss {
                    value: "a/b/v2".into(),
                    files: 2
                },
            ]
        );
    }

    #[test]
    fn live_rules_are_not_probed() {
        let rules: Vec<Box<dyn Operation>> = vec![Box::new(LineImport("a/b"))];
        let mut report = RunReport::with_rules(rules.iter().map(|r| r.to_string()));
        scanned(&mut report, "one.fake");
        report.rules[0].files_matched = 1;
        probe(&rules, &mut report, |_| {
            panic!("a live rule must not be probed")
        });
        assert!(report.rules[0].near_misses.is_empty());
    }
}
