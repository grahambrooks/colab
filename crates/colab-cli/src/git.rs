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
            return Err(Error::Config(format!(
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
            .map(|out| !out.trim().is_empty())
            .unwrap_or(false)
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

    pub fn head(&self) -> Result<String> {
        Ok(self.run(&["rev-parse", "HEAD"], &[])?.trim().to_string())
    }

    /// Undo the last commit, keeping its changes staged.
    pub fn uncommit(&self) -> Result<()> {
        self.run(&["reset", "-q", "--soft", "HEAD~1"], &[])
            .map(drop)
    }
}
