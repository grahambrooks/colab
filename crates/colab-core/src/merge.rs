//! Three-way merge of a run's result with a file that changed on disk
//! after the run read it.
//!
//! Rules in one script never need merging: each sees the previous one's
//! output, and their logical clashes are [`Operation::conflicts`]. The
//! versions that can diverge are the file as the run read it (the *base*),
//! as the run leaves it (*ours*), and as it is on disk when the run comes
//! to write (*theirs*: an editor save, another agent, a formatter during a
//! long `--verify`). [`prepare_writes`] re-reads every file before any is
//! written: an untouched file gets the run's text; a changed one gets the
//! [`three_way`] merge when the two sides' edits do not overlap; and
//! overlapping edits are a [`Conflict`], so the run writes nothing.
//!
//! [`Operation::conflicts`]: crate::Operation::conflicts

use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use similar::{DiffOp, TextDiff};

use crate::error::{Error, Result};
use crate::report::Conflict;

/// The outcome of [`three_way`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merge {
    /// Both sides' edits applied.
    Clean(String),
    /// Edits that overlap: the text cannot be merged without choosing.
    Conflicted(Vec<Hunk>),
}

/// One place both sides edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// 1-based line in the base where the overlapping edits start.
    pub line: usize,
    /// What our side has there.
    pub ours: String,
    /// What their side has there.
    pub theirs: String,
}

/// An edit to the base: the base lines it replaces, and their replacement.
#[derive(Debug, Clone)]
struct Edit<'a> {
    base: Range<usize>,
    lines: Vec<&'a str>,
}

fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn edits<'a>(base: &[&str], side: &[&'a str]) -> Vec<Edit<'a>> {
    TextDiff::from_slices(base, side)
        .ops()
        .iter()
        .filter(|op| !matches!(op, DiffOp::Equal { .. }))
        .map(|op| Edit {
            base: op.old_range(),
            lines: side[op.new_range()].to_vec(),
        })
        .collect()
}

/// Merge `ours` and `theirs`, both edits of `base`, line by line.
///
/// Edits that overlap or touch (as in git, two edits on adjacent lines
/// count as overlapping) conflict unless they are the same edit.
pub fn three_way(base: &str, ours: &str, theirs: &str) -> Merge {
    if ours == theirs || theirs == base {
        return Merge::Clean(ours.to_string());
    }
    if ours == base {
        return Merge::Clean(theirs.to_string());
    }
    let base_lines = lines(base);
    let (ours_lines, theirs_lines) = (lines(ours), lines(theirs));
    let (a, b) = (
        edits(&base_lines, &ours_lines),
        edits(&base_lines, &theirs_lines),
    );
    let touches = |x: &Range<usize>, y: &Range<usize>| x.start <= y.end && y.start <= x.end;

    let mut out = String::with_capacity(ours.len().max(theirs.len()));
    let mut hunks = Vec::new();
    let (mut i, mut j, mut at) = (0, 0, 0);
    while i < a.len() || j < b.len() {
        // The next group: an edit, plus every edit from either side that
        // touches the group as it grows.
        let first_ours = j >= b.len() || (i < a.len() && a[i].base.start <= b[j].base.start);
        let mut span = if first_ours {
            a[i].base.clone()
        } else {
            b[j].base.clone()
        };
        let (from_i, from_j) = (i, j);
        loop {
            if i < a.len() && touches(&a[i].base, &span) {
                span = span.start.min(a[i].base.start)..span.end.max(a[i].base.end);
                i += 1;
            } else if j < b.len() && touches(&b[j].base, &span) {
                span = span.start.min(b[j].base.start)..span.end.max(b[j].base.end);
                j += 1;
            } else {
                break;
            }
        }
        for line in &base_lines[at..span.start] {
            out.push_str(line);
        }
        let side = |edits: &[Edit<'_>], base_lines: &[&str]| {
            // The group's base span with this side's edits applied.
            let mut text = String::new();
            let mut pos = span.start;
            for edit in edits {
                for line in &base_lines[pos..edit.base.start] {
                    text.push_str(line);
                }
                for line in &edit.lines {
                    text.push_str(line);
                }
                pos = edit.base.end;
            }
            for line in &base_lines[pos..span.end] {
                text.push_str(line);
            }
            text
        };
        let (ours_part, theirs_part) = (
            side(&a[from_i..i], &base_lines),
            side(&b[from_j..j], &base_lines),
        );
        if from_i == i {
            out.push_str(&theirs_part);
        } else if from_j == j || ours_part == theirs_part {
            out.push_str(&ours_part);
        } else {
            hunks.push(Hunk {
                line: span.start + 1,
                ours: ours_part,
                theirs: theirs_part,
            });
        }
        at = span.end;
    }
    if !hunks.is_empty() {
        return Merge::Conflicted(hunks);
    }
    for line in &base_lines[at..] {
        out.push_str(line);
    }
    Merge::Clean(out)
}

/// One file [`prepare_writes`] cleared for writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub path: PathBuf,
    /// What to write: the run's text, or its merge with the disk's.
    pub text: String,
    /// The file as it is on disk now, which a backup must keep (it is
    /// what `colab undo` restores), not the text the run read.
    pub on_disk: String,
    /// A new file (`create`): nothing was on disk, and writing it makes
    /// its directories too.
    pub created: bool,
}

/// What [`prepare_writes`] decided for a run's changes.
#[derive(Debug, Default)]
pub struct Prepared {
    /// Each file to write.
    pub writes: Vec<Write>,
    /// Files that changed on disk since the run read them and were merged.
    pub merged: Vec<PathBuf>,
    /// Files whose disk changes overlap the run's: not written.
    pub conflicts: Vec<Conflict>,
}

impl Prepared {
    /// A warning for the report naming the merged files, if any.
    pub fn merge_warning(&self) -> Option<String> {
        (!self.merged.is_empty()).then(|| {
            let names: Vec<String> = self
                .merged
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            format!(
                "{} changed on disk after colab read it; merged with the run's changes: {}",
                if names.len() == 1 { "1 file" } else { "files" },
                names.join(", ")
            )
        })
    }
}

/// Check each change — `(path, as the run read it, as the run leaves it)`
/// — against the file on disk now, before anything is written.
pub fn prepare_writes<'a>(
    changes: impl IntoIterator<Item = (&'a Path, &'a str, &'a str)>,
) -> Result<Prepared> {
    let mut prepared = Prepared::default();
    for (path, before, after) in changes {
        let disk = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                prepared.conflicts.push(Conflict {
                    path: path.to_path_buf(),
                    rule: None,
                    message: "the file was deleted after colab read it".into(),
                });
                continue;
            }
            Err(e) => return Err(Error::io_at(path, e)),
        };
        if disk == before {
            prepared.writes.push(Write {
                path: path.to_path_buf(),
                text: after.to_string(),
                on_disk: disk,
                created: false,
            });
            continue;
        }
        match three_way(before, after, &disk) {
            Merge::Clean(text) => {
                // Written even when the merge equals the disk (their side
                // already holds our edits): every cleared file has a
                // `Write`, so a caller never falls back to its own text.
                prepared.merged.push(path.to_path_buf());
                prepared.writes.push(Write {
                    path: path.to_path_buf(),
                    text,
                    on_disk: disk,
                    created: false,
                });
            }
            Merge::Conflicted(hunks) => {
                let lines: Vec<String> = hunks.iter().map(|h| h.line.to_string()).collect();
                prepared.conflicts.push(Conflict {
                    path: path.to_path_buf(),
                    rule: None,
                    message: format!(
                        "the file changed on disk after colab read it, and the change overlaps \
                         the run's at line{} {}; rerun to start from the new text",
                        if lines.len() == 1 { "" } else { "s" },
                        lines.join(", ")
                    ),
                });
            }
        }
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "a\nb\nc\nd\ne\n";

    #[test]
    fn edits_far_apart_merge() {
        let ours = "A\nb\nc\nd\ne\n";
        let theirs = "a\nb\nc\nd\nE\n";
        assert_eq!(
            three_way(BASE, ours, theirs),
            Merge::Clean("A\nb\nc\nd\nE\n".into())
        );
    }

    #[test]
    fn insertions_and_deletions_merge() {
        let ours = "a\nb\nnew\nc\nd\ne\n";
        let theirs = "a\nb\nc\nd\n";
        assert_eq!(
            three_way(BASE, ours, theirs),
            Merge::Clean("a\nb\nnew\nc\nd\n".into())
        );
    }

    #[test]
    fn the_same_edit_on_both_sides_merges() {
        let both = "a\nB\nc\nd\ne\n";
        assert_eq!(three_way(BASE, both, both), Merge::Clean(both.into()));
        let ours = "a\nB\nc\nd\nE\n";
        assert_eq!(three_way(BASE, ours, both), Merge::Clean(ours.into()));
    }

    #[test]
    fn one_side_unchanged_takes_the_other() {
        let edited = "a\nx\ne\n";
        assert_eq!(three_way(BASE, BASE, edited), Merge::Clean(edited.into()));
        assert_eq!(three_way(BASE, edited, BASE), Merge::Clean(edited.into()));
    }

    #[test]
    fn overlapping_or_adjacent_edits_conflict() {
        let ours = "a\nours\nc\nd\ne\n";
        let theirs = "a\ntheirs\nc\nd\ne\n";
        let Merge::Conflicted(hunks) = three_way(BASE, ours, theirs) else {
            panic!("the same line, edited two ways");
        };
        assert_eq!(
            hunks,
            [Hunk {
                line: 2,
                ours: "ours\n".into(),
                theirs: "theirs\n".into()
            }]
        );
        let adjacent = "a\nb\nC\nd\ne\n";
        assert!(matches!(
            three_way(BASE, ours, adjacent),
            Merge::Conflicted(_)
        ));
    }

    #[test]
    fn a_missing_trailing_newline_survives() {
        let base = "a\nb\nc";
        assert_eq!(
            three_way(base, "A\nb\nc", "a\nb\nC"),
            Merge::Clean("A\nb\nC".into())
        );
    }

    #[test]
    fn prepare_writes_merges_reports_and_leaves_alone() {
        let dir = std::env::temp_dir().join(format!("colab-merge-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (same, moved, clash, gone) = (
            dir.join("same"),
            dir.join("moved"),
            dir.join("clash"),
            dir.join("gone"),
        );
        fs::write(&same, BASE).unwrap();
        fs::write(&moved, "a\nb\nc\nd\nE\n").unwrap();
        fs::write(&clash, "a\ntheirs\nc\nd\ne\n").unwrap();
        let ours = "a\nours\nc\nd\ne\n";
        let prepared = prepare_writes([
            (same.as_path(), BASE, ours),
            (moved.as_path(), BASE, ours),
            (clash.as_path(), BASE, ours),
            (gone.as_path(), BASE, ours),
        ])
        .unwrap();
        let writes: Vec<_> = prepared
            .writes
            .iter()
            .map(|w| (w.path.clone(), w.text.as_str(), w.on_disk.as_str()))
            .collect();
        assert_eq!(
            writes,
            [
                (same.clone(), ours, BASE),
                (moved.clone(), "a\nours\nc\nd\nE\n", "a\nb\nc\nd\nE\n"),
            ]
        );
        assert_eq!(prepared.merged, [moved]);
        let paths: Vec<_> = prepared.conflicts.iter().map(|c| c.path.clone()).collect();
        assert_eq!(paths, [clash, gone]);
        assert!(prepared.conflicts[0].message.contains("line 2"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
