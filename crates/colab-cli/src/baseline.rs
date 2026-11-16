//! `--baseline <ref>`: report only the findings that are new since a git
//! ref, so a convention can gate a repository whose old code breaks it.
//!
//! The script runs first over the tree as it was at the ref (a detached
//! worktree, removed afterwards), and each finding it would make there is
//! remembered by its file, its rules, the lines it replaces and what it
//! puts in their place, so a finding that only moved still matches. Each
//! file of the real run then keeps only its findings the ref did not
//! have: its new text is rebuilt from those alone, so every output, the
//! diff, JSON, SARIF and GitHub annotations, shows just them, and a file
//! left with none is unchanged. A run with a baseline never writes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use colab_core::render;
use colab_core::walker::FileChange;
use colab_core::{Error, Result, RunReport};

/// A finding as the baseline knows it.
type Key = (PathBuf, Vec<String>, String, String);

/// The findings at the ref, counted, so two identical findings in a file
/// at the ref excuse two now.
#[derive(Debug, Default)]
pub struct Baseline {
    pub reference: String,
    keys: HashMap<Key, usize>,
    /// Findings left out of the run because the ref had them.
    pub dropped: usize,
}

/// Removes the baseline worktree however the run ends.
struct Worktree<'a> {
    repo: &'a crate::git::Repo,
    dir: PathBuf,
}

impl Drop for Worktree<'_> {
    fn drop(&mut self) {
        if let Err(e) = self.repo.remove_worktree(&self.dir) {
            log::warn!(
                "could not remove the baseline worktree {}: {e}",
                self.dir.display()
            );
        }
    }
}

impl Baseline {
    /// The findings `walk` makes when run from the directory that, at
    /// `reference`, is the working directory now. `walk` gets a visitor
    /// for each file and the report naming the rules.
    pub fn collect(
        reference: &str,
        report: &RunReport,
        walk: impl FnOnce(&mut dyn FnMut(FileChange) -> Result<()>) -> Result<()>,
    ) -> Result<Self> {
        let cwd = std::env::current_dir().map_err(crate::cli::io_to_error)?;
        let repo = crate::git::Repo::discover(&cwd).ok_or_else(|| {
            Error::Git("--baseline compares with a git ref, so it needs a git work tree".into())
        })?;
        let canonical = cwd.canonicalize().unwrap_or(cwd.clone());
        let prefix = canonical
            .strip_prefix(repo.top())
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "colab-baseline-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        repo.add_worktree(&dir, reference).map_err(|e| match e {
            Error::Config(why) => Error::Config(format!("--baseline {reference}: {why}")),
            Error::Git(why) => Error::Git(format!("--baseline {reference}: {why}")),
            other => other,
        })?;
        let worktree = Worktree { repo: &repo, dir };
        let mut baseline = Self {
            reference: reference.to_string(),
            ..Self::default()
        };
        let there = worktree.dir.join(&prefix);
        let walked = if there.is_dir() {
            std::env::set_current_dir(&there).map_err(|e| Error::io_at(&there, e))?;
            let walked = walk(&mut |change: FileChange| {
                for key in keys(&change, report) {
                    *baseline.keys.entry(key).or_default() += 1;
                }
                Ok(())
            });
            std::env::set_current_dir(&cwd).map_err(|e| Error::io_at(&cwd, e))?;
            walked
        } else {
            // The directory did not exist at the ref: everything is new.
            Ok(())
        };
        drop(worktree);
        walked.map(|()| baseline)
    }

    /// `change` with only its findings the ref did not have.
    pub fn filter(&mut self, change: FileChange, report: &RunReport) -> FileChange {
        if !change.changed() {
            return change;
        }
        let blocks = render::blocks(&change, None);
        let mut kept = Vec::with_capacity(blocks.len());
        for key in keys(&change, report) {
            match self.keys.get_mut(&key) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    self.dropped += 1;
                    kept.push(false);
                }
                _ => kept.push(true),
            }
        }
        if kept.iter().all(|k| *k) {
            return change;
        }
        let old: Vec<&str> = change.before.split_inclusive('\n').collect();
        let new: Vec<&str> = change.after.split_inclusive('\n').collect();
        let mut after = String::with_capacity(change.after.len());
        let mut at = 0;
        for (block, keep) in blocks.iter().zip(&kept) {
            if !keep {
                continue;
            }
            after.extend(old[at..block.old.start].iter().copied());
            after.extend(new[block.new.clone()].iter().copied());
            at = block.old.end;
        }
        after.extend(old[at..].iter().copied());
        let any = kept.iter().any(|k| *k);
        FileChange {
            rules_fired: if any { change.rules_fired } else { Vec::new() },
            conflicts: if any { change.conflicts } else { Vec::new() },
            after,
            ..change
        }
    }
}

/// Each finding of `change`, as a [`Key`], in block order.
fn keys(change: &FileChange, report: &RunReport) -> Vec<Key> {
    let old: Vec<&str> = change.before.split_inclusive('\n').collect();
    let new: Vec<&str> = change.after.split_inclusive('\n').collect();
    let path = change
        .path
        .strip_prefix("./")
        .unwrap_or(&change.path)
        .to_path_buf();
    render::blocks(change, None)
        .into_iter()
        .map(|block| {
            let mut rules: Vec<String> = block
                .rules
                .iter()
                .filter_map(|&i| report.rules.get(i).map(|r| r.rule.clone()))
                .collect();
            rules.sort();
            (
                path.clone(),
                rules,
                old.get(block.old.clone())
                    .map(<[&str]>::concat)
                    .unwrap_or_default(),
                new.get(block.new.clone())
                    .map(<[&str]>::concat)
                    .unwrap_or_default(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(before: &str, after: &str) -> FileChange {
        FileChange {
            path: "./a.go".into(),
            before: before.into(),
            after: after.into(),
            rules_fired: vec![0],
            conflicts: Vec::new(),
        }
    }

    #[test]
    fn findings_the_ref_had_are_dropped_even_after_lines_move() {
        let report = RunReport::with_rules(["r"]);
        let old = change("x\nold\ny\n", "x\nnew\ny\n");
        let mut baseline = Baseline::default();
        for key in keys(&old, &report) {
            *baseline.keys.entry(key).or_default() += 1;
        }
        // The same finding two lines lower, and a second, new one.
        let now = change("a\nb\nx\nold\ny\nold\n", "a\nb\nx\nnew\ny\nnew\n");
        let filtered = baseline.filter(now, &report);
        assert_eq!(filtered.after, "a\nb\nx\nold\ny\nnew\n");
        assert_eq!(baseline.dropped, 1);
        // Nothing new: the file is unchanged, and credits no rule.
        let mut again = Baseline::default();
        for key in keys(&old, &report) {
            *again.keys.entry(key).or_default() += 1;
        }
        let same = again.filter(change("x\nold\ny\n", "x\nnew\ny\n"), &report);
        assert!(!same.changed() && same.rules_fired.is_empty());
    }
}
