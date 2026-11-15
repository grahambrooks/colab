//! `colab fleet <fleet.toml>`: one migration across many repositories,
//! each an ordinary colab run.
//!
//! ```toml
//! [fleet]
//! script = "migrate.colab"     # relative to this file; or `fix = true`
//! branch = "colab/errors-to-fmt" # what --write commits on
//! title = "Move from pkg/errors to fmt"
//! body = "Made by colab fleet."
//! canary = "api"                 # runs first; default the first repo
//!
//! [[repos]]
//! name = "api"
//! path = "../api"                # a local checkout
//!
//! [[repos]]
//! git = "https://github.com/acme/billing"   # cloned into the cache
//! ref = "main"                   # the base; default the default branch
//! paths = ["src"]                # where the script runs; default "."
//! ```
//!
//! Each repository's run happens in a temporary `git worktree` at its
//! base, so a local checkout is never touched: colab runs there as a
//! subprocess (`refactor --script …`, or `fix` with the repository's own
//! `colab.toml`), and its JSON report is kept. Without `--write` that is
//! all. With `--write`, a change is committed on `branch` (a branch that
//! already exists is refused, never overwritten); with `--pr` the branch
//! is pushed and `gh pr create` opens the PR. The canary runs first, and
//! if it fails nothing else runs. `colab fleet status` reports each
//! branch's PR as `gh pr view` sees it. There is no coordinator: the
//! manifest and git are the state.
use std::fmt::Write as _;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Parser, Subcommand, ValueEnum};
use colab_core::{Error, Result};
use serde_json::{Value, json};

#[derive(Parser, Debug)]
pub struct FleetArgs {
    #[command(subcommand)]
    command: Option<FleetCommand>,

    /// The fleet manifest.
    manifest: Option<PathBuf>,

    /// Commit each repository's change on the manifest's `branch`.
    #[arg(long)]
    write: bool,

    /// With --write: push the branch and open a PR with `gh pr create`.
    #[arg(long, requires = "write")]
    pr: bool,

    /// Only these repositories (by `name`), canary rules still applying.
    /// Repeatable.
    #[arg(long = "repo", value_name = "NAME")]
    repos: Vec<String>,

    /// `human`: a line per repository. `json`: one document.
    #[arg(long, value_enum, default_value_t = FleetFormat::Human)]
    format: FleetFormat,
}

#[derive(Subcommand, Debug)]
enum FleetCommand {
    /// Each repository's PR for the manifest's branch, as `gh pr view`
    /// reports it.
    Status {
        manifest: PathBuf,
        #[arg(long, value_enum, default_value_t = FleetFormat::Human)]
        format: FleetFormat,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum FleetFormat {
    Human,
    Json,
}

/// What a fleet runs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Run {
    Script(PathBuf),
    Fix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Repo {
    name: String,
    source: Source,
    base: Option<String>,
    paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Path(PathBuf),
    Git(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Manifest {
    run: Run,
    branch: String,
    title: String,
    body: String,
    canary: Option<String>,
    repos: Vec<Repo>,
}

fn bad(path: &Path, why: impl std::fmt::Display) -> Error {
    Error::Config(format!("{}: {why}", path.display()))
}

fn parse(path: &Path, text: &str) -> Result<Manifest> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| bad(path, e))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let fleet = doc
        .get("fleet")
        .and_then(|f| f.as_table())
        .ok_or_else(|| bad(path, "a fleet manifest has a `[fleet]` table"))?;
    for (key, _) in fleet.iter() {
        if !["script", "fix", "branch", "title", "body", "canary"].contains(&key) {
            return Err(bad(
                path,
                format!(
                    "unknown key `fleet.{key}`; it has script or fix, branch, title, body, canary"
                ),
            ));
        }
    }
    let text_of = |key: &str| fleet.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let fix = fleet
        .get("fix")
        .and_then(toml_edit::Item::as_bool)
        .unwrap_or(false);
    let run = match (text_of("script"), fix) {
        (Some(script), false) => Run::Script(dir.join(script)),
        (None, true) => Run::Fix,
        _ => {
            return Err(bad(
                path,
                "`[fleet]` runs one thing: `script = \"<file>\"`, or `fix = true` for each repository's colab.toml",
            ));
        }
    };
    let branch = text_of("branch").ok_or_else(|| bad(path, "`[fleet]` needs `branch`"))?;
    let title = text_of("title").unwrap_or_else(|| format!("colab: {branch}"));
    let body = text_of("body").unwrap_or_else(|| "Made with `colab fleet`.".into());
    let tables = doc
        .get("repos")
        .and_then(|r| r.as_array_of_tables())
        .ok_or_else(|| bad(path, "a fleet manifest lists `[[repos]]`"))?;
    let mut repos = Vec::new();
    for table in tables.iter() {
        for (key, _) in table.iter() {
            if !["name", "path", "git", "ref", "paths"].contains(&key) {
                return Err(bad(
                    path,
                    format!("unknown key `repos.{key}`; a repo has name, path or git, ref, paths"),
                ));
            }
        }
        let get = |key: &str| table.get(key).and_then(|v| v.as_str()).map(str::to_string);
        let source = match (get("path"), get("git")) {
            (Some(p), None) => Source::Path(dir.join(p)),
            (None, Some(url)) => Source::Git(url),
            _ => return Err(bad(path, "each `[[repos]]` has one of `path` or `git`")),
        };
        let name = get("name").unwrap_or_else(|| match &source {
            Source::Path(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Source::Git(url) => url
                .trim_end_matches('/')
                .trim_end_matches(".git")
                .rsplit('/')
                .next()
                .unwrap_or(url)
                .to_string(),
        });
        let paths = match table.get("paths") {
            None => vec![".".into()],
            Some(v) => v
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .ok_or_else(|| bad(path, "`paths` is a list of strings"))?,
        };
        if repos.iter().any(|r: &Repo| r.name == name) {
            return Err(bad(
                path,
                format!("two repos are named `{name}`; give one a `name`"),
            ));
        }
        repos.push(Repo {
            name,
            source,
            base: get("ref"),
            paths,
        });
    }
    let canary = text_of("canary");
    if let Some(c) = &canary
        && !repos.iter().any(|r| &r.name == c)
    {
        return Err(bad(
            path,
            format!("the canary `{c}` is not one of the repos"),
        ));
    }
    Ok(Manifest {
        run,
        branch,
        title,
        body,
        canary,
        repos,
    })
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| Error::Io {
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
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The repository to run in, and the base commit-ish there.
fn checkout(repo: &Repo) -> Result<(PathBuf, String)> {
    match &repo.source {
        Source::Path(path) => {
            let path = path.canonicalize().map_err(|e| Error::io_at(path, e))?;
            Ok((path, repo.base.clone().unwrap_or_else(|| "HEAD".into())))
        }
        Source::Git(url) => {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            url.hash(&mut hasher);
            let dir = crate::git_packs::cache_dir()
                .join("fleet")
                .join(format!("{:016x}", hasher.finish()));
            if dir.join(".git").exists() {
                git(&dir, &["fetch", "-q", "origin"])?;
            } else {
                fs::create_dir_all(dir.parent().unwrap_or(&dir))
                    .map_err(|e| Error::io_at(&dir, e))?;
                let out = Command::new("git")
                    .args(["clone", "-q", "--no-checkout", url])
                    .arg(&dir)
                    .output()
                    .map_err(|e| Error::Io {
                        path: None,
                        source: e,
                    })?;
                if !out.status.success() {
                    return Err(Error::Config(format!(
                        "cloning {url}: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
            }
            let base = match &repo.base {
                Some(r) => format!("origin/{r}"),
                None => "origin/HEAD".into(),
            };
            Ok((dir, base))
        }
    }
}

/// One repository's outcome.
#[derive(Debug, Clone)]
struct Outcome {
    name: String,
    /// `changed`, `unchanged`, `failed`, or `skipped` (after a failed
    /// canary).
    status: &'static str,
    files: Vec<String>,
    branch: Option<String>,
    pr: Option<String>,
    error: Option<String>,
}

impl Outcome {
    fn failed(name: &str, error: impl std::fmt::Display) -> Self {
        Self {
            name: name.into(),
            status: "failed",
            files: Vec::new(),
            branch: None,
            pr: None,
            error: Some(error.to_string()),
        }
    }
}

/// An error's own words, without its kind (`configuration error: `).
fn words(e: &Error) -> String {
    match e {
        Error::Config(m) | Error::UnsupportedOperation(m) => m.clone(),
        other => other.to_string(),
    }
}

/// Removes a fleet worktree however the repository's run ends.
struct Worktree {
    repo: PathBuf,
    dir: PathBuf,
}

impl Drop for Worktree {
    fn drop(&mut self) {
        let dir = self.dir.to_string_lossy().into_owned();
        if git(&self.repo, &["worktree", "remove", "--force", &dir]).is_err() {
            fs::remove_dir_all(&self.dir).ok();
            git(&self.repo, &["worktree", "prune"]).ok();
        }
    }
}

fn run_one(manifest: &Manifest, repo: &Repo, write: bool, pr: bool) -> Outcome {
    match try_one(manifest, repo, write, pr) {
        Ok(outcome) => outcome,
        Err(e) => Outcome::failed(&repo.name, words(&e)),
    }
}

fn try_one(manifest: &Manifest, repo: &Repo, write: bool, pr: bool) -> Result<Outcome> {
    let (root, base) = checkout(repo)?;
    let branch_exists = git(
        &root,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{}", manifest.branch),
        ],
    )
    .is_ok();
    if write && branch_exists {
        return Err(Error::Config(format!(
            "the branch `{}` already exists in {}; delete it or name another in the manifest",
            manifest.branch,
            root.display()
        )));
    }
    let dir = std::env::temp_dir().join(format!(
        "colab-fleet-{}-{}-{}",
        repo.name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let dir_text = dir.to_string_lossy().into_owned();
    if write {
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &manifest.branch,
                &dir_text,
                &base,
            ],
        )?;
    } else {
        git(
            &root,
            &["worktree", "add", "-q", "--detach", &dir_text, &base],
        )?;
    }
    let worktree = Worktree {
        repo: root.clone(),
        dir: dir.clone(),
    };

    let exe = std::env::current_exe().map_err(crate::cli::io_to_error)?;
    let mut command = Command::new(exe);
    command.current_dir(&dir);
    match &manifest.run {
        Run::Script(script) => {
            let script = script.canonicalize().map_err(|e| Error::io_at(script, e))?;
            command.arg("refactor").arg("--script").arg(script);
        }
        Run::Fix => {
            command.arg("fix");
        }
    }
    command.args(["--format", "json"]);
    command.arg(if write { "--write" } else { "--dry-run" });
    if matches!(manifest.run, Run::Script(_)) {
        command.arg("--");
        command.args(&repo.paths);
    }
    let out = command.output().map_err(crate::cli::io_to_error)?;
    let report: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    if !out.status.success() || report.is_null() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // The run's last error line, without its log stamp and kind.
        let line = stderr
            .lines()
            .rev()
            .find(|l| l.contains("ERROR") || l.contains("error"))
            .unwrap_or("colab failed")
            .trim();
        let line = line.split_once("] ").map_or(line, |(_, rest)| rest);
        let why = line
            .split_once(": ")
            .filter(|(kind, _)| kind.ends_with("error") || kind.ends_with("operation"))
            .map_or(line, |(_, rest)| rest)
            .to_string();
        if write {
            drop(worktree);
            git(&root, &["branch", "-D", &manifest.branch]).ok();
        }
        return Err(Error::Config(why));
    }
    let files: Vec<String> = report["changed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str())
        .map(|p| p.trim_start_matches("./").to_string())
        .collect();
    let mut outcome = Outcome {
        name: repo.name.clone(),
        status: if files.is_empty() {
            "unchanged"
        } else {
            "changed"
        },
        files,
        branch: None,
        pr: None,
        error: None,
    };
    if !write {
        return Ok(outcome);
    }
    if outcome.files.is_empty() {
        drop(worktree);
        git(&root, &["branch", "-D", &manifest.branch]).ok();
        return Ok(outcome);
    }
    git(&dir, &["add", "-A"])?;
    git(
        &dir,
        &["commit", "-q", "-m", &manifest.title, "-m", &manifest.body],
    )?;
    outcome.branch = Some(manifest.branch.clone());
    if pr {
        git(&dir, &["push", "-q", "-u", "origin", &manifest.branch])?;
        let out = Command::new("gh")
            .current_dir(&dir)
            .args([
                "pr",
                "create",
                "--head",
                &manifest.branch,
                "--title",
                &manifest.title,
                "--body",
                &manifest.body,
            ])
            .output()
            .map_err(|e| Error::Config(format!("running `gh pr create`: {e}")))?;
        if !out.status.success() {
            return Err(Error::Config(format!(
                "`gh pr create` failed (the branch `{}` is pushed): {}",
                manifest.branch,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        outcome.pr = Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    Ok(outcome)
}

pub fn run(args: FleetArgs) -> Result<u8> {
    if let Some(FleetCommand::Status { manifest, format }) = args.command {
        return status(&manifest, format);
    }
    let Some(path) = args.manifest else {
        return Err(Error::Config(
            "name the fleet manifest: `colab fleet <fleet.toml>`".into(),
        ));
    };
    let text = fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))?;
    let manifest = parse(&path, &text)?;
    let mut repos: Vec<&Repo> = manifest
        .repos
        .iter()
        .filter(|r| args.repos.is_empty() || args.repos.contains(&r.name))
        .collect();
    for name in &args.repos {
        if !manifest.repos.iter().any(|r| &r.name == name) {
            return Err(Error::Config(format!(
                "--repo {name}: the manifest has no such repo"
            )));
        }
    }
    // The canary first.
    let canary = manifest
        .canary
        .clone()
        .or_else(|| repos.first().map(|r| r.name.clone()));
    if let Some(c) = &canary
        && let Some(at) = repos.iter().position(|r| &r.name == c)
    {
        let first = repos.remove(at);
        repos.insert(0, first);
    }
    let mut outcomes = Vec::new();
    let mut stopped = false;
    for (i, repo) in repos.iter().enumerate() {
        if stopped {
            outcomes.push(Outcome {
                status: "skipped",
                error: Some("the canary failed".into()),
                ..Outcome::failed(&repo.name, "")
            });
            continue;
        }
        let outcome = run_one(&manifest, repo, args.write, args.pr);
        if i == 0 && outcome.status == "failed" && repos.len() > 1 {
            stopped = true;
        }
        outcomes.push(outcome);
    }
    let failed = outcomes.iter().any(|o| o.status == "failed");
    let mut out = std::io::stdout().lock();
    match args.format {
        FleetFormat::Json => {
            let repos: Vec<Value> = outcomes
                .iter()
                .map(|o| {
                    json!({
                        "repo": o.name, "status": o.status, "files": o.files,
                        "branch": o.branch, "pr": o.pr, "error": o.error,
                    })
                })
                .collect();
            writeln!(
                out,
                "{}",
                json!({"branch": manifest.branch, "repos": repos})
            )
        }
        FleetFormat::Human => {
            let mut text = String::new();
            for o in &outcomes {
                let detail = match (o.status, &o.pr, &o.error) {
                    (_, Some(pr), _) => pr.clone(),
                    (_, _, Some(e)) if !e.is_empty() => e.clone(),
                    ("changed", ..) => format!("{} file(s)", o.files.len()),
                    _ => String::new(),
                };
                let _ = writeln!(text, "{:<10} {:<20} {detail}", o.status, o.name);
            }
            write!(out, "{text}")
        }
    }
    .map_err(crate::cli::io_to_error)?;
    Ok(if failed { 1 } else { 0 })
}

fn status(path: &Path, format: FleetFormat) -> Result<u8> {
    let text = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
    let manifest = parse(path, &text)?;
    let mut rows = Vec::new();
    for repo in &manifest.repos {
        let row = match checkout(repo) {
            Err(e) => json!({"repo": repo.name, "error": e.to_string()}),
            Ok((root, _)) => {
                let out = Command::new("gh")
                    .current_dir(&root)
                    .args([
                        "pr",
                        "view",
                        &manifest.branch,
                        "--json",
                        "url,state,isDraft,mergeable,reviewDecision",
                    ])
                    .output();
                match out {
                    Ok(o) if o.status.success() => {
                        let mut pr: Value =
                            serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
                        pr["repo"] = json!(repo.name);
                        pr
                    }
                    Ok(o) => {
                        json!({"repo": repo.name, "state": "NONE", "error": String::from_utf8_lossy(&o.stderr).trim()})
                    }
                    Err(e) => json!({"repo": repo.name, "error": format!("running `gh`: {e}")}),
                }
            }
        };
        rows.push(row);
    }
    let mut out = std::io::stdout().lock();
    match format {
        FleetFormat::Json => writeln!(out, "{}", json!({"branch": manifest.branch, "repos": rows})),
        FleetFormat::Human => {
            let mut text = String::new();
            for r in &rows {
                let _ = writeln!(
                    text,
                    "{:<20} {:<8} {}",
                    r["repo"].as_str().unwrap_or(""),
                    r["state"].as_str().unwrap_or("?"),
                    r["url"].as_str().or(r["error"].as_str()).unwrap_or("")
                );
            }
            write!(out, "{text}")
        }
    }
    .map_err(crate::cli::io_to_error)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_names_one_run_a_branch_and_its_repos() {
        let path = Path::new("/w/fleet.toml");
        let m = parse(
            path,
            "[fleet]\nscript = \"m.colab\"\nbranch = \"colab/m\"\ncanary = \"api\"\n\n[[repos]]\nname = \"api\"\npath = \"../api\"\n\n[[repos]]\ngit = \"https://example.test/acme/billing.git\"\nref = \"main\"\npaths = [\"src\"]\n",
        )
        .unwrap();
        assert_eq!(m.run, Run::Script("/w/m.colab".into()));
        assert_eq!(m.repos[1].name, "billing");
        assert_eq!(m.repos[1].paths, ["src"]);
        assert_eq!(m.title, "colab: colab/m");
        let err = |text: &str| parse(path, text).unwrap_err().to_string();
        assert!(
            err("[fleet]\nbranch = \"b\"\n[[repos]]\npath = \"a\"\n").contains("runs one thing")
        );
        assert!(err("[fleet]\nfix = true\n[[repos]]\npath = \"a\"\n").contains("needs `branch`"));
        assert!(
            err("[fleet]\nfix = true\nbranch = \"b\"\ncanary = \"x\"\n[[repos]]\npath = \"a\"\n")
                .contains("canary `x`")
        );
        assert!(err("[fleet]\nfix = true\nbranch = \"b\"\n[[repos]]\npath = \"a\"\n[[repos]]\npath = \"b/a\"\n").contains("two repos are named `a`"));
    }
}
