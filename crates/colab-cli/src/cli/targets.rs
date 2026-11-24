//! What a run walks: the paths named, or the files `git` reports as changed
//! since a ref or staged.

use super::*;

/// The walk options `args`' include / exclude / ignore / jobs flags ask for.
pub(super) fn walk_options(args: &RefactorArgs) -> WalkOptions {
    WalkOptions {
        include: args.include.clone(),
        exclude: args.exclude.clone(),
        respect_gitignore: !args.no_ignore,
        follow_symlinks: false,
        jobs: resolve_jobs(args.jobs),
    }
}

/// Where a run over `targets` reads its files: the files `git` reports
/// (`--changed-since`, `--staged`), or a walk of the targets.
pub(super) fn source(args: &RefactorArgs, targets: &[PathBuf]) -> Result<Source> {
    Ok(if let Some(reference) = args.changed_since.as_deref() {
        Source::Paths(git_changed_since(reference)?)
    } else if args.staged {
        Source::Paths(git_staged()?)
    } else {
        Source::Walk {
            targets: targets.to_vec(),
            options: walk_options(args),
        }
    })
}

/// One transformer (a single rule, for the runs that apply rules one at a
/// time) over the run's [`source`], then the semantic gate.
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
    let outcome = source(args, targets)?.visit(transformer, visit)?;
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
