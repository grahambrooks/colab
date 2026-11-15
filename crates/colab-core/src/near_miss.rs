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
use crate::report::{NearMiss, RunReport, Unchanged};
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
    // Every dead rule is probed for why it left matches unchanged; those
    // with a target are also probed for values resembling it.
    let dead: Vec<(usize, Option<&str>)> = report
        .dead_rules()
        .filter_map(|stat| Some((stat.index, rules.get(stat.index)?.near_miss_target())))
        .collect();
    // A rule that changed some files may still have left matches alone in
    // others (a template that would not parse there, a match outside its
    // scope); it is probed for that reason only.
    let live: Vec<usize> = report
        .rules
        .iter()
        .filter(|stat| stat.files_matched > 0 && stat.index < rules.len())
        .map(|stat| stat.index)
        .collect();
    if dead.is_empty() && live.is_empty() {
        return;
    }

    // rule index -> observed value -> number of files it appeared in.
    let mut seen: BTreeMap<usize, BTreeMap<String, u64>> = BTreeMap::new();
    // rule index -> (first reason given, files that gave one).
    let mut unchanged: BTreeMap<usize, (String, u64)> = BTreeMap::new();
    let mut files_read = 0;
    for path in &report.scanned_sample {
        if files_read >= PROBE_FILE_LIMIT {
            break;
        }
        let relevant: Vec<(usize, bool)> = dead
            .iter()
            .filter(|&&(index, _)| rules[index].is_file_relevant(path))
            .map(|&(index, target)| (index, target.is_some()))
            .collect();
        // A file the rule changed is not one it skipped. (Read alone on the
        // original, a rule that builds on an earlier rule's output would
        // look as if it had.)
        let changed_here = report.fired.get(path);
        let live_here: Vec<usize> = live
            .iter()
            .copied()
            .filter(|&index| rules[index].is_file_relevant(path))
            .filter(|index| !changed_here.is_some_and(|f| f.contains(index)))
            .collect();
        if relevant.is_empty() && live_here.is_empty() {
            continue;
        }
        let Some(source) = read(path) else {
            continue;
        };
        files_read += 1;
        // A rule that asks a language server asks it about this file.
        crate::semantic::at_path(path, || {
            for index in live_here {
                let rule = &rules[index];
                if rule
                    .prefilter()
                    .is_some_and(|literal| !source.contains(literal))
                {
                    continue;
                }
                // After a write the file holds the rule's own result, so "already
                // as the rule asks" is its success, not a skip.
                if let Some(reason) = rule
                    .why_unchanged(&source)
                    .filter(|r| !r.starts_with(crate::ALREADY))
                {
                    unchanged.entry(index).or_insert((reason, 0)).1 += 1;
                }
            }
            for (index, has_target) in relevant {
                if let Some(reason) = rules[index].why_unchanged(&source) {
                    unchanged.entry(index).or_insert((reason, 0)).1 += 1;
                }
                if !has_target {
                    continue;
                }
                let values: BTreeSet<String> =
                    rules[index].observed_targets(&source).into_iter().collect();
                let counts = seen.entry(index).or_default();
                for value in values {
                    *counts.entry(value).or_default() += 1;
                }
            }
        });
    }

    for index in live {
        if let Some((message, files)) = unchanged.remove(&index) {
            report.rules[index].unchanged = Some(Unchanged { message, files });
        }
    }
    for (index, target) in dead {
        if let Some((message, files)) = unchanged.remove(&index) {
            report.rules[index].unchanged = Some(Unchanged { message, files });
        }
        let (Some(target), Some(counts)) = (target, seen.remove(&index)) else {
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
            conflicts: Vec::new(),
        });
    }

    #[test]
    fn dead_rule_gets_the_closest_observed_values() {
        let rules: Vec<Box<dyn Operation>> = vec![Box::new(LineImport("a/b"))];
        let mut report = RunReport::with_rules(rules.iter().map(std::string::ToString::to_string));
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
    fn a_file_a_live_rule_changed_is_not_probed() {
        // A rule that builds on an earlier rule's output reads, alone on
        // the original, as if it skipped the file it changed.
        let rules: Vec<Box<dyn Operation>> = vec![Box::new(LineImport("a/b"))];
        let mut report = RunReport::with_rules(rules.iter().map(std::string::ToString::to_string));
        scanned(&mut report, "one.fake");
        report.rules[0].files_matched = 1;
        report.fired.insert("one.fake".into(), vec![0]);
        probe(&rules, &mut report, |_| {
            panic!("a file the rule changed is not read")
        });
        assert!(report.rules[0].unchanged.is_none());
    }

    #[test]
    fn live_rules_get_no_near_misses() {
        // A rule that changed files is probed only for matches it left
        // alone (`why_unchanged`), never for values resembling its target.
        let rules: Vec<Box<dyn Operation>> = vec![Box::new(LineImport("a/b"))];
        let mut report = RunReport::with_rules(rules.iter().map(std::string::ToString::to_string));
        scanned(&mut report, "one.fake");
        report.rules[0].files_matched = 1;
        probe(&rules, &mut report, |_| Some("import a/b/v2\n".into()));
        assert!(report.rules[0].near_misses.is_empty());
        assert!(report.rules[0].unchanged.is_none());
    }
}
