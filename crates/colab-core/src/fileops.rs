//! File operations: moving, creating, and deleting whole files.
//!
//! Text edits are [`FileChange`]s. A language server can also ask for a
//! file to be renamed (a Java class and its file, a Rust module), created,
//! or deleted. Those are [`FileOp`]s, collected with the edits into one
//! [`ChangeSet`] that describes the end state of a refactoring.
//!
//! colab makes file operations through git — `git mv`, `git add`,
//! `git rm` — so history and undo survive. That lives in the CLI; this
//! module only describes them, so the shared renderer can report them.

use std::path::{Path, PathBuf};

use crate::walker::FileChange;

/// One operation on a whole file (or, for a move, a directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    /// Rename `from` to `to`.
    Move { from: PathBuf, to: PathBuf },
    /// Create an empty `path`; its content, if any, is an edit.
    Create { path: PathBuf },
    /// Remove `path`, whose content was `before`.
    Delete { path: PathBuf, before: String },
}

impl FileOp {
    /// Every path the operation touches.
    pub fn paths(&self) -> Vec<&Path> {
        match self {
            FileOp::Move { from, to } => vec![from, to],
            FileOp::Create { path } | FileOp::Delete { path, .. } => vec![path],
        }
    }

    /// The operation with every path passed through `f` — used to report
    /// paths relative to where the user is.
    pub fn map_paths(&self, mut f: impl FnMut(&Path) -> PathBuf) -> FileOp {
        match self {
            FileOp::Move { from, to } => FileOp::Move {
                from: f(from),
                to: f(to),
            },
            FileOp::Create { path } => FileOp::Create { path: f(path) },
            FileOp::Delete { path, before } => FileOp::Delete {
                path: f(path),
                before: before.clone(),
            },
        }
    }
}

impl std::fmt::Display for FileOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileOp::Move { from, to } => write!(f, "move {} -> {}", from.display(), to.display()),
            FileOp::Create { path } => write!(f, "create {}", path.display()),
            FileOp::Delete { path, .. } => write!(f, "delete {}", path.display()),
        }
    }
}

/// The end state of one refactoring: file operations, in the order they
/// are made, and the text edits to make after them. An edit's `path` is
/// where the file ends up; its `before` is the file's content before the
/// refactoring, wherever it was then (see [`ChangeSet::origin_of`]).
#[derive(Debug, Clone, Default)]
pub struct ChangeSet {
    pub ops: Vec<FileOp>,
    pub edits: Vec<FileChange>,
}

impl ChangeSet {
    /// Text edits only.
    pub fn from_edits(edits: Vec<FileChange>) -> Self {
        Self {
            ops: Vec::new(),
            edits,
        }
    }

    /// Where the file now at `path` was before the operations ran, or
    /// `None` if one of them created it. Unmoved files are their own
    /// origin.
    pub fn origin_of(&self, path: &Path) -> Option<PathBuf> {
        let mut current = path.to_path_buf();
        for op in self.ops.iter().rev() {
            match op {
                FileOp::Move { from, to } => {
                    if let Ok(rest) = current.strip_prefix(to) {
                        current = if rest.as_os_str().is_empty() {
                            from.clone()
                        } else {
                            from.join(rest)
                        };
                    }
                }
                FileOp::Create { path } if *path == current => return None,
                _ => {}
            }
        }
        Some(current)
    }

    /// Every path touched by an operation or an edit, at both ends of a
    /// move, deduplicated.
    pub fn touched_paths(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .ops
            .iter()
            .flat_map(|op| op.paths().into_iter().map(Path::to_path_buf))
            .collect();
        for edit in &self.edits {
            out.push(edit.path.clone());
            if let Some(origin) = self.origin_of(&edit.path) {
                out.push(origin);
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

/// How alike two versions of a file are, from 0.0 to 1.0, by lines.
///
/// Git records no renames; it infers them when a deleted and an added
/// file are at least 50% similar. A move whose content also changes past
/// that point shows up as a delete plus an add, and `git log --follow`
/// stops there.
pub fn similarity(before: &str, after: &str) -> f32 {
    if before == after {
        return 1.0;
    }
    similar::TextDiff::from_lines(before, after).ratio()
}

/// Below this, git is unlikely to pair a moved file with its old path.
pub const RENAME_SIMILARITY: f32 = 0.5;

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(path: &str, before: &str, after: &str) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            before: before.into(),
            after: after.into(),
            rules_fired: vec![0],
            conflicts: Vec::new(),
        }
    }

    #[test]
    fn origin_follows_file_and_directory_moves_backwards() {
        let set = ChangeSet {
            ops: vec![
                FileOp::Move {
                    from: "/r/a.rs".into(),
                    to: "/r/b.rs".into(),
                },
                FileOp::Move {
                    from: "/r/src".into(),
                    to: "/r/lib".into(),
                },
                FileOp::Create {
                    path: "/r/new.rs".into(),
                },
            ],
            edits: vec![],
        };
        assert_eq!(set.origin_of(Path::new("/r/b.rs")), Some("/r/a.rs".into()));
        assert_eq!(
            set.origin_of(Path::new("/r/lib/x.rs")),
            Some("/r/src/x.rs".into())
        );
        assert_eq!(set.origin_of(Path::new("/r/new.rs")), None);
        assert_eq!(set.origin_of(Path::new("/r/c.rs")), Some("/r/c.rs".into()));
    }

    #[test]
    fn touched_paths_cover_both_ends_of_a_move_and_edit_origins() {
        let set = ChangeSet {
            ops: vec![FileOp::Move {
                from: "/r/a.rs".into(),
                to: "/r/b.rs".into(),
            }],
            edits: vec![edit("/r/b.rs", "x", "y"), edit("/r/c.rs", "x", "y")],
        };
        let paths: Vec<PathBuf> = vec!["/r/a.rs".into(), "/r/b.rs".into(), "/r/c.rs".into()];
        assert_eq!(set.touched_paths(), paths);
    }

    #[test]
    fn similarity_is_one_for_identical_text_and_drops_with_rewrites() {
        assert_eq!(similarity("a\nb\n", "a\nb\n"), 1.0);
        assert!(similarity("a\nb\nc\nd\n", "a\nb\nc\nx\n") >= RENAME_SIMILARITY);
        assert!(similarity("a\nb\nc\nd\n", "w\nx\ny\nz\n") < RENAME_SIMILARITY);
    }
}
