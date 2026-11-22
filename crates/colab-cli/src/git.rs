//! The few git commands colab's file operations need.
//!
//! Every command runs as `git -C <top-level>` with absolute paths, so it
//! behaves the same from any directory, in a repository or a linked
//! worktree.

use std::path::{Path, PathBuf};
use std::process::Command;

use colab_core::{Error, Result};

/// A git work tree, identified by its top-level directory.
#[derive(Debug, Clone)]
pub struct Repo {
    top: PathBuf,
}

impl Repo {
    /// The work tree containing `dir`, if any. `dir` need not exist yet
    /// (the parent of a move target); the nearest existing ancestor is
    /// asked.
    pub fn discover(dir: &Path) -> Option<Self> {
        let existing = dir.ancestors().find(|d| d.is_dir())?;
        let out = Command::new("git")
            .arg("-C")
            .arg(existing)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let top = String::from_utf8(out.stdout).ok()?.trim().to_string();
        let top = PathBuf::from(top);
        Some(Self {
            top: top.canonicalize().unwrap_or(top),
        })
    }

    pub fn top(&self) -> &Path {
        &self.top
    }

    fn run(&self, args: &[&str], paths: &[&Path]) -> Result<String> {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.top).args(args);
        if !paths.is_empty() {
            command.arg("--").args(paths);
        }
        let out = command.output().map_err(|e| Error::Io {
            path: None,
            source: e,
        })?;
        if !out.status.success() {
            return Err(Error::Git(format!(
                "`git {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Whether git tracks `path` (a file, or any file under a directory).
    pub fn is_tracked(&self, path: &Path) -> bool {
        self.run(&["ls-files"], &[path])
            .is_ok_and(|out| !out.trim().is_empty())
    }

    /// The paths among `paths` with uncommitted changes — staged or not —
    /// as `git status --porcelain` lines. Untracked files are left out:
    /// they hold no history, and moving one is undone by moving it back.
    pub fn dirty(&self, paths: &[PathBuf]) -> Result<Vec<String>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let out = self.run(&["status", "--porcelain=v1", "--untracked-files=no"], &refs)?;
        Ok(out
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect())
    }

    /// `git mv from to`.
    pub fn mv(&self, from: &Path, to: &Path) -> Result<()> {
        self.run(&["mv"], &[from, to]).map(drop)
    }

    /// `git add path`.
    pub fn add(&self, path: &Path) -> Result<()> {
        self.run(&["add"], &[path]).map(drop)
    }

    /// `git rm path` — removed from the index and the disk.
    pub fn rm(&self, path: &Path) -> Result<()> {
        self.run(&["rm", "-q", "-f"], &[path]).map(drop)
    }

    /// Commit exactly `paths` as they are in the work tree (`--only`), so
    /// whatever else the user has staged stays staged and uncommitted.
    /// Returns the new commit's id.
    pub fn commit_only(&self, message: &str, paths: &[&Path]) -> Result<String> {
        self.run(&["commit", "-q", "--only", "-m", message], paths)?;
        self.head()
    }

    /// Run git with `GIT_INDEX_FILE` set to `index`, feeding `stdin`.
    fn run_in(&self, index: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<String> {
        use std::io::Write;
        use std::process::Stdio;
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.top)
            .args(args)
            .env("GIT_INDEX_FILE", index)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let io = |e| Error::Io {
            path: None,
            source: e,
        };
        let mut child = command.spawn().map_err(io)?;
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(data).map_err(io)?;
        }
        let out = child.wait_with_output().map_err(io)?;
        if !out.status.success() {
            return Err(Error::Git(format!(
                "`git {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Whether the branch `name` exists.
    pub fn has_branch(&self, name: &str) -> bool {
        self.run(
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
            &[],
        )
        .is_ok()
    }

    /// Whether `name` is a valid branch name.
    pub fn valid_branch_name(&self, name: &str) -> bool {
        self.run(&["check-ref-format", "--branch", name], &[])
            .is_ok()
    }

    /// A commit on `base` whose tree is `base`'s with each `(path, content)`
    /// replaced, made through a private index: the work tree, the real
    /// index and the current branch are untouched. Paths are relative to
    /// the top level; each keeps its mode in `base`. Returns the commit id.
    pub fn commit_on(&self, base: &str, files: &[(String, &str)], message: &str) -> Result<String> {
        let index = std::env::temp_dir().join(format!(
            "colab-split-{}-{}.index",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or_default(|d| d.as_nanos())
        ));
        let result = (|| {
            self.run_in(&index, &["read-tree", base], None)?;
            for (path, content) in files {
                let mode = self
                    .run_in(&index, &["ls-files", "-s", "--", path], None)?
                    .split_whitespace()
                    .next()
                    .unwrap_or("100644")
                    .to_string();
                let blob = self.run_in(
                    &index,
                    &["hash-object", "-w", "--stdin"],
                    Some(content.as_bytes()),
                )?;
                self.run_in(
                    &index,
                    &[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        &format!("{mode},{blob},{path}"),
                    ],
                    None,
                )?;
            }
            let tree = self.run_in(&index, &["write-tree"], None)?;
            self.run_in(
                &index,
                &["commit-tree", &tree, "-p", base, "-m", message],
                None,
            )
        })();
        let _ = std::fs::remove_file(&index);
        result
    }

    /// Point a new branch `name` at `commit`; fails if it already exists
    /// (the empty old value makes `update-ref` create-only).
    pub fn create_branch(&self, name: &str, commit: &str) -> Result<()> {
        self.run(
            &["update-ref", &format!("refs/heads/{name}"), commit, ""],
            &[],
        )
        .map(drop)
    }

    pub fn head(&self) -> Result<String> {
        Ok(self.run(&["rev-parse", "HEAD"], &[])?.trim().to_string())
    }

    /// Check `reference` out, detached, into a new worktree at `dir`.
    pub fn add_worktree(&self, dir: &Path, reference: &str) -> Result<()> {
        let dir = dir.to_string_lossy();
        self.run(
            &["worktree", "add", "--detach", "--quiet", &dir, reference],
            &[],
        )
        .map(|_| ())
    }

    /// Remove the worktree at `dir`, made by [`add_worktree`](Self::add_worktree).
    pub fn remove_worktree(&self, dir: &Path) -> Result<()> {
        let dir = dir.to_string_lossy();
        self.run(&["worktree", "remove", "--force", &dir], &[])
            .map(|_| ())
    }

    /// Undo the last commit, keeping its changes staged.
    pub fn uncommit(&self) -> Result<()> {
        self.run(&["reset", "-q", "--soft", "HEAD~1"], &[])
            .map(drop)
    }
}
