//! What a run walks: the paths named, or the files `git` reports as changed
//! since a ref or staged.

use super::*;

/// Iterate `paths` directly (used by `--changed-since` and
/// `--staged`), invoking the visitor for files the transformer
/// considers relevant. Skips paths that no longer exist on disk
/// — `git diff` may report files that have been deleted.
pub(super) fn run_against_paths<T, F>(
    transformer: &T,
    paths: &[PathBuf],
    visit: &mut F,
) -> Result<WalkOutcome>
where
    T: CodeTransformer + Sync,
    F: FnMut(FileChange) -> Result<()>,
{
    let mut outcome = WalkOutcome::default();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        outcome.files_visited += 1;
        if !transformer.is_file_relevant(path) {
            continue;
        }
        let before = match fs::read_to_string(path) {
            Ok(before) => before,
            // Match the tree walk: an unreadable file is recorded, not fatal.
            Err(e) => {
                outcome.skipped.push(colab_core::SkippedFile {
                    path: path.clone(),
                    reason: e.to_string(),
                });
                continue;
            }
        };
        let applied = transformer.apply_at(path, &before);
        visit(FileChange::from_outcome(path.clone(), before, applied))?;
    }
    Ok(outcome)
}

/// Drive a single transformer through the chosen file source
/// (`--changed-since` / `--staged` / tree walk). Pulled out so
/// the per-rule loop can reuse the same source-selection logic.
/// Walk `targets` with `args`' include / exclude / ignore settings.
pub(super) fn walk_targets<T, F>(
    transformer: &T,
    args: &RefactorArgs,
    targets: &[PathBuf],
    visit: &mut F,
) -> Result<WalkOutcome>
where
    T: CodeTransformer + Sync,
    F: FnMut(FileChange) -> Result<()> + ?Sized,
{
    let opts = WalkOptions {
        include: args.include.clone(),
        exclude: args.exclude.clone(),
        respect_gitignore: !args.no_ignore,
        follow_symlinks: false,
        jobs: resolve_jobs(args.jobs),
    };
    let mut outcome = WalkOutcome::default();
    for target in targets {
        let one = walker::walk_with(transformer, target, &opts, &mut |c| visit(c))?;
        outcome.files_visited += one.files_visited;
        outcome.skipped.extend(one.skipped);
    }
    Ok(outcome)
}

pub(super) fn run_transformer<T, F>(
    transformer: &T,
    args: &RefactorArgs,
    targets: &[PathBuf],
    visit: &mut F,
) -> Result<WalkOutcome>
where
    T: CodeTransformer + Sync,
    F: FnMut(FileChange) -> Result<()>,
{
    let outcome = if let Some(ref_) = args.changed_since.as_deref() {
        let files = git_changed_since(ref_)?;
        run_against_paths(transformer, &files, visit)
    } else if args.staged {
        let files = git_staged()?;
        run_against_paths(transformer, &files, visit)
    } else {
        walk_targets(transformer, args, targets, visit)
    }?;
    semantic_failure()?;
    Ok(outcome)
}

pub(super) fn git_changed_since(reference: &str) -> Result<Vec<PathBuf>> {
    git_paths(&["diff", "--name-only", "--diff-filter=ACMRT", reference])
}

pub(super) fn git_staged() -> Result<Vec<PathBuf>> {
    git_paths(&["diff", "--name-only", "--cached", "--diff-filter=ACMRT"])
}

pub(super) fn git_paths(args: &[&str]) -> Result<Vec<PathBuf>> {
    let output = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|e| Error::Config(format!("could not invoke git ({}): {}", args.join(" "), e)))?;
    if !output.status.success() {
        return Err(Error::Config(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect())
}
