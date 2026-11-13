//! Make a [`ChangeSet`] real: file operations through git, then text edits.
//!
//! An agent works in a git repository or worktree, where a mistake can be
//! undone. So file operations are made only there, and only with git:
//! `git mv` for a move (the index records it at once), `git add` for a
//! created file, `git rm` for a deleted one. Text edits are written after
//! the operations and left unstaged, so `git diff --staged` shows the file
//! operations and `git diff` the content changes.
//!
//! Git stores no renames; it infers them from content similarity. A file
//! moved *and* heavily rewritten in one commit looks like a delete plus an
//! add, and `git log --follow` / `blame` stop at the move. `--commit-moves`
//! commits the operations on their own first; without it, such files are
//! warned about.

use std::fs;
use std::path::{Path, PathBuf};

use colab_core::fileops::{ChangeSet, FileOp, RENAME_SIMILARITY, similarity};
use colab_core::{Error, Result};
use serde_json::{Value, json};

use crate::git::Repo;

/// Name of the file-operation log inside a `--backup` directory.
pub const MANIFEST: &str = "colab-file-ops.json";

/// How to apply.
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Skip the "touched paths have no uncommitted changes" check.
    pub allow_dirty: bool,
    /// Commit the file operations on their own before writing edits.
    pub commit_moves: bool,
    /// Where to save pre-change contents and the operation log.
    pub backup: Option<PathBuf>,
}

/// A change set that passed its preconditions.
#[derive(Debug)]
pub struct Prepared {
    /// The work tree, when the change set has file operations.
    pub repo: Option<Repo>,
    /// Warnings for the report: untracked moves, history at risk.
    pub warnings: Vec<String>,
}

/// One operation as made, with what undoing it needs.
#[derive(Debug, Clone)]
pub struct Done {
    pub op: FileOp,
    /// Whether git tracked the file when the operation was made.
    pub tracked: bool,
}

/// What [`apply`] did, for [`revert`].
#[derive(Debug, Default)]
pub struct Applied {
    pub done: Vec<Done>,
    /// The `--commit-moves` commit, if one was made.
    pub commit: Option<String>,
}

/// Check that `set` can be applied, in every mode — so a dry run tells
/// the truth about a write.
///
/// Text-only change sets need nothing. A set with file operations needs a
/// git work tree, and (unless `allow_dirty`) no uncommitted changes in any
/// path it touches, so that undoing colab can never discard the user's
/// own work.
pub fn prepare(set: &ChangeSet, options: &ApplyOptions) -> Result<Prepared> {
    let mut warnings = Vec::new();
    if set.ops.is_empty() {
        return Ok(Prepared {
            repo: None,
            warnings,
        });
    }
    let anchor = set.ops[0].paths()[0];
    let repo = Repo::discover(anchor.parent().unwrap_or(anchor)).ok_or_else(|| {
        Error::UnsupportedOperation(format!(
            "{} is not in a git work tree; colab makes file operations ({}) only with git, so history and undo survive",
            anchor.display(),
            set.ops[0]
        ))
    })?;
    if !options.allow_dirty {
        let dirty = repo.dirty(&set.touched_paths())?;
        if !dirty.is_empty() {
            return Err(Error::Config(format!(
                "these paths have uncommitted changes, which undoing this could lose: {}; commit or stash them, or pass --allow-dirty",
                dirty.join(", ")
            )));
        }
    }
    for op in &set.ops {
        if let FileOp::Move { from, .. } = op
            && from.exists()
            && !repo.is_tracked(from)
        {
            warnings.push(format!(
                "{} is untracked; it is moved on disk only (git has no history to keep)",
                relative(&repo, from).display()
            ));
        }
    }
    if !options.commit_moves {
        for edit in &set.edits {
            let Some(origin) = set.origin_of(&edit.path) else {
                continue;
            };
            if origin == edit.path {
                continue;
            }
            let alike = similarity(&edit.before, &edit.after);
            if alike < RENAME_SIMILARITY {
                warnings.push(format!(
                    "{} -> {} is only {:.0}% similar after its edits, so git may record a delete and an add and `git log --follow` would stop here; pass --commit-moves to commit the move on its own first",
                    relative(&repo, &origin).display(),
                    relative(&repo, &edit.path).display(),
                    alike * 100.0
                ));
            }
        }
    }
    Ok(Prepared {
        repo: Some(repo),
        warnings,
    })
}

/// Make the file operations, optionally commit them, then write the edits.
/// `snapshot(path, content)` saves a file's pre-change content (the
/// `--backup` mirror); it is called with each edited file's *original*
/// path, so `colab undo` restores contents after moving files back.
pub fn apply<S>(
    set: &ChangeSet,
    prepared: &Prepared,
    options: &ApplyOptions,
    mut snapshot: S,
) -> Result<Applied>
where
    S: FnMut(&Path, &str) -> Result<()>,
{
    let mut applied = Applied::default();
    let planned = |repo: &Repo| -> Vec<Done> {
        set.ops
            .iter()
            .map(|op| Done {
                op: op.clone(),
                tracked: op.paths().first().is_some_and(|p| repo.is_tracked(p)),
            })
            .collect()
    };
    // Logged before anything moves, so an interrupted run can be undone.
    if let (Some(repo), Some(dir)) = (&prepared.repo, &options.backup) {
        write_manifest(dir, repo, &planned(repo))?;
    }
    if options.backup.is_some() {
        for edit in &set.edits {
            if let Some(origin) = set.origin_of(&edit.path) {
                snapshot(&origin, &edit.before)?;
            }
        }
    }

    if let Some(repo) = &prepared.repo {
        for op in &set.ops {
            // Asked as each operation runs: a file created a moment ago is
            // tracked by the time it is moved.
            let tracked = op.paths().first().is_some_and(|p| repo.is_tracked(p));
            make(repo, op, tracked)?;
            applied.done.push(Done {
                op: op.clone(),
                tracked,
            });
        }
        if let Some(dir) = &options.backup {
            write_manifest(dir, repo, &applied.done)?;
        }
        if options.commit_moves {
            let paths: Vec<&Path> = set.ops.iter().flat_map(FileOp::paths).collect();
            let message = commit_message(repo, &set.ops);
            applied.commit = Some(repo.commit_only(&message, &paths)?);
        }
    }

    for edit in &set.edits {
        if let Some(parent) = edit.path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
        }
        fs::write(&edit.path, &edit.after).map_err(|e| Error::io_at(&edit.path, e))?;
    }
    Ok(applied)
}

/// Put everything back after a failed `--verify`: drop the
/// `--commit-moves` commit if it is still `HEAD`, undo the operations in
/// reverse, then restore each edited file's original content at its
/// original path.
pub fn revert(set: &ChangeSet, prepared: &Prepared, applied: &Applied) -> Result<()> {
    if let Some(repo) = &prepared.repo {
        if let Some(commit) = &applied.commit
            && repo.head()? == *commit
        {
            repo.uncommit()?;
        }
        reverse(repo, &applied.done)?;
    }
    for edit in &set.edits {
        // A created file has no origin: it went when its `Create` was undone.
        if let Some(origin) = set.origin_of(&edit.path) {
            fs::write(&origin, &edit.before).map_err(|e| Error::io_at(&origin, e))?;
        }
    }
    Ok(())
}

/// The undo half of `colab undo` for a backup that holds a manifest:
/// reverse its operations, before the content mirror is restored.
/// Returns how many operations were undone.
pub fn undo_manifest(backup: &Path) -> Result<usize> {
    let path = backup.join(MANIFEST);
    if !path.exists() {
        return Ok(0);
    }
    let text = fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))?;
    let manifest: Value = serde_json::from_str(&text)
        .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
    let top = manifest["root"]
        .as_str()
        .ok_or_else(|| Error::Config(format!("{}: no `root`", path.display())))?;
    let repo = Repo::discover(Path::new(top))
        .ok_or_else(|| Error::Config(format!("{top} is no longer a git work tree")))?;
    let done: Vec<Done> = manifest["ops"]
        .as_array()
        .into_iter()
        .flatten()
        .map(done_from_json)
        .collect::<Result<_>>()?;
    reverse(&repo, &done)?;
    Ok(done.len())
}

fn make(repo: &Repo, op: &FileOp, tracked: bool) -> Result<()> {
    match op {
        FileOp::Move { from, to } => {
            create_parent(to)?;
            if tracked {
                repo.mv(from, to)
            } else {
                fs::rename(from, to).map_err(|e| Error::io_at(from, e))
            }
        }
        FileOp::Create { path } => {
            create_parent(path)?;
            fs::write(path, "").map_err(|e| Error::io_at(path, e))?;
            repo.add(path)
        }
        FileOp::Delete { path, .. } => {
            if tracked {
                repo.rm(path)
            } else {
                fs::remove_file(path).map_err(|e| Error::io_at(path, e))
            }
        }
    }
}

/// Undo `done`, last first.
fn reverse(repo: &Repo, done: &[Done]) -> Result<()> {
    for Done { op, tracked } in done.iter().rev() {
        match op {
            FileOp::Move { from, to } => {
                create_parent(from)?;
                if *tracked {
                    repo.mv(to, from)?;
                } else {
                    fs::rename(to, from).map_err(|e| Error::io_at(to, e))?;
                }
                remove_empty_parents(to, repo.top());
            }
            FileOp::Create { path } => {
                if path.exists() {
                    repo.rm(path)?;
                }
                remove_empty_parents(path, repo.top());
            }
            FileOp::Delete { path, before } => {
                create_parent(path)?;
                fs::write(path, before).map_err(|e| Error::io_at(path, e))?;
                if *tracked {
                    repo.add(path)?;
                }
            }
        }
    }
    Ok(())
}

fn create_parent(path: &Path) -> Result<()> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e)),
        None => Ok(()),
    }
}

/// Remove directories a move or create left empty, up to (not including)
/// the work tree's top.
fn remove_empty_parents(path: &Path, top: &Path) {
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == top || !d.starts_with(top) || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// `path` relative to the work tree's top, for messages.
fn relative<'p>(repo: &Repo, path: &'p Path) -> &'p Path {
    path.strip_prefix(repo.top()).unwrap_or(path)
}

fn commit_message(repo: &Repo, ops: &[FileOp]) -> String {
    let ops: Vec<FileOp> = ops
        .iter()
        .map(|op| op.map_paths(|p| relative(repo, p).to_path_buf()))
        .collect();
    match ops.as_slice() {
        [op] => format!("colab: {op}"),
        _ => {
            let body: Vec<String> = ops.iter().map(|op| format!("- {op}")).collect();
            format!(
                "colab: {} file operations\n\n{}",
                ops.len(),
                body.join("\n")
            )
        }
    }
}

fn write_manifest(dir: &Path, repo: &Repo, done: &[Done]) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| Error::io_at(dir, e))?;
    let ops: Vec<Value> = done.iter().map(done_to_json).collect();
    let manifest = json!({"version": 1, "root": repo.top(), "ops": ops});
    let path = dir.join(MANIFEST);
    fs::write(&path, manifest.to_string()).map_err(|e| Error::io_at(&path, e))
}

fn done_to_json(done: &Done) -> Value {
    match &done.op {
        FileOp::Move { from, to } => {
            json!({"op": "move", "from": from, "to": to, "tracked": done.tracked})
        }
        FileOp::Create { path } => json!({"op": "create", "path": path, "tracked": done.tracked}),
        FileOp::Delete { path, before } => {
            json!({"op": "delete", "path": path, "before": before, "tracked": done.tracked})
        }
    }
}

fn done_from_json(value: &Value) -> Result<Done> {
    let path = |key: &str| -> Result<PathBuf> {
        value[key]
            .as_str()
            .map(PathBuf::from)
            .ok_or_else(|| Error::Config(format!("{MANIFEST}: operation without `{key}`")))
    };
    let op = match value["op"].as_str() {
        Some("move") => FileOp::Move {
            from: path("from")?,
            to: path("to")?,
        },
        Some("create") => FileOp::Create {
            path: path("path")?,
        },
        Some("delete") => FileOp::Delete {
            path: path("path")?,
            before: value["before"].as_str().unwrap_or("").to_string(),
        },
        other => {
            return Err(Error::Config(format!(
                "{MANIFEST}: unknown operation {other:?}"
            )));
        }
    };
    Ok(Done {
        op,
        tracked: value["tracked"].as_bool().unwrap_or(false),
    })
}
