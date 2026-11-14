//! M23: the agent eval suite.
//!
//! Each task under `evals/tasks/<name>/` is a change an agent is asked to
//! make (`task.md`), the files before (`input/`) and the files that must
//! change, as they must end up (`expected/`), and the colab commands that
//! make it (`colab.sh`, one per line, with any script they use beside it).
//!
//! For every task this test
//!
//! 1. runs the commands in a temporary git repository holding `input/`,
//!    and checks the result exactly: every expected file matches, every
//!    other file is unchanged, and files listed as `deleted` are gone;
//! 2. measures what colab cost an agent: the bytes of each command, of
//!    each script it had to write, and of everything the command printed;
//! 3. models what the same change costs as `Read` + `Edit` calls, from
//!    the diff between `input/` and `expected/` (the same model as
//!    `scripts/price-edits.py`); and
//! 4. fails if the colab cost exceeds the task's `budget`, so output that
//!    grows is caught, or if, over all tasks, colab costs more than the
//!    model.
//!
//! A task whose `meta` says `requires = <tool>` is skipped when the tool
//! is not on `PATH`. `COLAB_BLESS=1` rewrites `evals/results.md`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/colab-cli is two levels down")
        .to_path_buf()
}

/// Every file under `dir`, relative, with its contents.
fn files(dir: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == ".git" || name == "target" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(text) = fs::read_to_string(&path) {
                out.insert(path.strip_prefix(dir).unwrap().to_path_buf(), text);
            }
        }
    }
    out
}

struct Meta {
    budget: usize,
    requires: Vec<String>,
    deleted: Vec<PathBuf>,
}

fn meta(dir: &Path) -> Meta {
    let mut meta = Meta {
        budget: usize::MAX,
        requires: Vec::new(),
        deleted: Vec::new(),
    };
    for line in fs::read_to_string(dir.join("meta"))
        .unwrap_or_default()
        .lines()
    {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "budget" => meta.budget = value.parse().expect("budget is a number"),
            "requires" => meta.requires.push(value.to_string()),
            "deleted" => meta.deleted.push(PathBuf::from(value)),
            other => panic!("unknown meta key `{other}` in {}", dir.display()),
        }
    }
    meta
}

fn on_path(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "eval")
        .env("GIT_AUTHOR_EMAIL", "eval@example.com")
        .env("GIT_COMMITTER_NAME", "eval")
        .env("GIT_COMMITTER_EMAIL", "eval@example.com")
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

// ---------------------------------------------------------------------------
// The Read + Edit model (as scripts/price-edits.py)
// ---------------------------------------------------------------------------

const EDIT_OVERHEAD: usize = 150;
const EDIT_RESULT: usize = 110;
const READ_CALL: usize = 150;
const LINE_PREFIX: usize = 7;
const CONTEXT: usize = 12;
const GREP_CALL: usize = 200;
const GREP_LINE: usize = 110;
const SHELL_CALL: usize = 150;

#[derive(Default)]
struct Cost {
    bytes: usize,
    calls: usize,
}

/// One file's change as `Read` + `Edit`: a targeted read around its hunks
/// and an edit per hunk; returns the hunk count too.
fn price_file(before: &str, after: &str) -> (Cost, usize) {
    let diff = similar::TextDiff::from_lines(before, after);
    let old: Vec<&str> = before.split_inclusive('\n').collect();
    let groups = diff.grouped_ops(0);
    let mut cost = Cost::default();
    let mut window = std::collections::BTreeSet::new();
    for group in &groups {
        let (mut removed, mut added, mut start) = (0, 0, usize::MAX);
        for op in group {
            let r = op.old_range();
            start = start.min(r.start);
            removed += old[r.clone()].iter().map(|l| l.len()).sum::<usize>();
            let n = op.new_range();
            added += after
                .split_inclusive('\n')
                .skip(n.start)
                .take(n.len())
                .map(str::len)
                .sum::<usize>();
            window.extend(r.start.saturating_sub(CONTEXT)..(r.end + CONTEXT).min(old.len()));
        }
        let anchor = start
            .checked_sub(1)
            .and_then(|i| old.get(i))
            .map_or(0, |l| l.len());
        cost.bytes += EDIT_OVERHEAD + 2 * anchor + removed + added + EDIT_RESULT;
        cost.calls += 1;
    }
    if !groups.is_empty() {
        cost.bytes += READ_CALL
            + window
                .iter()
                .map(|&i| old[i].len() + LINE_PREFIX)
                .sum::<usize>();
        cost.calls += 1;
    }
    (cost, groups.len())
}

/// The whole task as an agent without colab would do it.
fn price_task(
    input: &BTreeMap<PathBuf, String>,
    expected: &BTreeMap<PathBuf, String>,
    deleted: &[PathBuf],
) -> Cost {
    let mut cost = Cost::default();
    let mut hunks = 0;
    for (path, after) in expected {
        match input.get(path) {
            Some(before) => {
                let (c, h) = price_file(before, after);
                cost.bytes += c.bytes;
                cost.calls += c.calls;
                hunks += h;
            }
            // A moved file is one `git mv` (then edited like any other);
            // a new one is a `Write` of its content.
            None => match deleted
                .iter()
                .find(|d| input.get(*d).is_some_and(|t| t == after))
            {
                Some(_) => {
                    cost.bytes += SHELL_CALL;
                    cost.calls += 1;
                }
                None => {
                    cost.bytes += EDIT_OVERHEAD + after.len() + EDIT_RESULT;
                    cost.calls += 1;
                }
            },
        }
    }
    // One search to find the sites.
    cost.bytes += GREP_CALL + GREP_LINE * hunks.max(1);
    cost.calls += 1;
    cost
}

// ---------------------------------------------------------------------------
// Running a task
// ---------------------------------------------------------------------------

struct Outcome {
    name: String,
    title: String,
    colab: Cost,
    model: Cost,
    budget: usize,
}

fn run_task(dir: &Path, bin_dir: &Path) -> Result<Outcome, String> {
    let name = dir.file_name().unwrap().to_string_lossy().to_string();
    let meta = meta(dir);
    let title = fs::read_to_string(dir.join("task.md"))
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or("")
        .trim_start_matches("# ")
        .to_string();
    let input = files(&dir.join("input"));
    let expected = files(&dir.join("expected"));

    let work = std::env::temp_dir().join(format!("colab-eval-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    for (path, text) in &input {
        let to = work.join(path);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::write(to, text).unwrap();
    }
    git(&work, &["init", "-q", "."]);
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-qm", "input"]);

    // What colab cost: each script the agent writes, each command, and
    // everything it prints (stdout and the log on stderr).
    let mut colab = Cost::default();
    for entry in fs::read_dir(dir).unwrap().flatten() {
        if entry.path().extension().is_some_and(|e| e == "codemod") {
            colab.bytes += EDIT_OVERHEAD + fs::read_to_string(entry.path()).unwrap().len();
            colab.calls += 1;
        }
    }
    let path_var = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    for line in fs::read_to_string(dir.join("colab.sh")).unwrap().lines() {
        if line.trim().is_empty() {
            continue;
        }
        let out = Command::new("sh")
            .arg("-c")
            .arg(line)
            .current_dir(&work)
            .env("PATH", &path_var)
            .env("TASK", dir)
            .output()
            .unwrap();
        colab.bytes += line.len() + out.stdout.len() + out.stderr.len();
        colab.calls += 1;
        if !out.status.success() {
            return Err(format!(
                "{name}: `{line}` failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }

    // The result, exactly.
    let after = files(&work);
    let mut wrong = Vec::new();
    for (path, want) in &expected {
        if after.get(path) != Some(want) {
            wrong.push(format!("{} differs from expected/", path.display()));
        }
    }
    for (path, before) in &input {
        if expected.contains_key(path) {
            continue;
        }
        let gone = meta.deleted.contains(path);
        match (after.get(path), gone) {
            (Some(_), true) => wrong.push(format!("{} should be gone", path.display())),
            (Some(now), false) if now != before => wrong.push(format!(
                "{} changed but is not in expected/",
                path.display()
            )),
            (None, false) => wrong.push(format!("{} disappeared", path.display())),
            _ => {}
        }
    }
    for path in after.keys() {
        if !input.contains_key(path) && !expected.contains_key(path) {
            wrong.push(format!(
                "{} appeared but is not in expected/",
                path.display()
            ));
        }
    }
    // A moved file keeps its history.
    for path in expected.keys().filter(|p| !input.contains_key(*p)) {
        if !meta.deleted.is_empty() {
            let log = Command::new("git")
                .args(["log", "--follow", "--format=%s", "--"])
                .arg(path)
                .current_dir(&work)
                .output()
                .unwrap();
            if !String::from_utf8_lossy(&log.stdout)
                .lines()
                .any(|l| l == "input")
            {
                wrong.push(format!("{} lost its history", path.display()));
            }
        }
    }
    let _ = fs::remove_dir_all(&work);
    if !wrong.is_empty() {
        return Err(format!("{name}: {}", wrong.join("; ")));
    }
    let model = price_task(&input, &expected, &meta.deleted);
    Ok(Outcome {
        name,
        title,
        colab,
        model,
        budget: meta.budget,
    })
}

fn results_table(outcomes: &[Outcome], skipped: &[(String, String)]) -> String {
    let mut out = String::from(
        "# Eval results\n\nGenerated by `COLAB_BLESS=1 cargo test -p colab-cli --test evals`; \
         see [`README.md`](README.md) for what is measured. Bytes are what\nthe agent sent and read; \
         tokens ≈ bytes / 4.\n\n\
         | Task | colab calls | colab bytes | Read + Edit calls | Read + Edit bytes | Saved |\n\
         | ---- | ----------: | ----------: | ----------------: | ----------------: | ----: |\n",
    );
    let (mut c, mut m, mut cc, mut mc) = (0, 0, 0, 0);
    for o in outcomes {
        c += o.colab.bytes;
        m += o.model.bytes;
        cc += o.colab.calls;
        mc += o.model.calls;
        out.push_str(&format!(
            "| {} ({}) | {} | {} | {} | {} | {}% |\n",
            o.title,
            o.name,
            o.colab.calls,
            o.colab.bytes,
            o.model.calls,
            o.model.bytes,
            saved(o.colab.bytes, o.model.bytes)
        ));
    }
    out.push_str(&format!(
        "| **Total** | **{cc}** | **{c}** | **{mc}** | **{m}** | **{}%** |\n",
        saved(c, m)
    ));
    if !skipped.is_empty() {
        out.push_str("\nSkipped (a required tool is not installed):\n\n");
        for (name, why) in skipped {
            out.push_str(&format!("- `{name}`: needs {why}\n"));
        }
    }
    out
}

fn saved(colab: usize, model: usize) -> i64 {
    100 - (100 * colab as i64) / (model.max(1) as i64)
}

#[test]
fn every_eval_task_is_correct_and_within_budget() {
    let root = repo_root();
    let bin = assert_cmd::cargo::cargo_bin("colab");
    let bin_dir = bin.parent().unwrap().to_path_buf();
    let mut dirs: Vec<PathBuf> = fs::read_dir(root.join("evals/tasks"))
        .expect("evals/tasks exists")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    let (mut outcomes, mut skipped, mut failures) = (Vec::new(), Vec::new(), Vec::new());
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let missing: Vec<String> = meta(dir)
            .requires
            .into_iter()
            .filter(|t| !on_path(t))
            .collect();
        if !missing.is_empty() {
            skipped.push((name, missing.join(", ")));
            continue;
        }
        match run_task(dir, &bin_dir) {
            Ok(o) => {
                if o.colab.bytes > o.budget {
                    failures.push(format!(
                        "{}: colab cost {} bytes, over its budget of {}",
                        o.name, o.colab.bytes, o.budget
                    ));
                }
                outcomes.push(o);
            }
            Err(e) => failures.push(e),
        }
    }
    let table = results_table(&outcomes, &skipped);
    println!("{table}");
    if std::env::var_os("COLAB_BLESS").is_some() {
        fs::write(root.join("evals/results.md"), &table).unwrap();
    }
    let colab: usize = outcomes.iter().map(|o| o.colab.bytes).sum();
    let model: usize = outcomes.iter().map(|o| o.model.bytes).sum();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(outcomes.len() >= 5, "too few tasks ran: {}", outcomes.len());
    assert!(
        colab < model,
        "over all tasks colab cost {colab} bytes, not less than the {model} modelled for Read + Edit"
    );
}
