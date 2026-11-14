//! Corpus-driven integration tests.
//!
//! Walks `tests/corpus/<lang>/<case>/` at the workspace root. Each case
//! supplies a `script`, an `input/` tree, and an `expected/` tree. The
//! harness compiles the script, applies the resulting transform to every
//! file in `input/`, and asserts:
//!
//! 1. The first pass output matches the file at the same relative path
//!    under `expected/`.
//! 2. Re-applying the transform to its own output is a no-op
//!    (idempotency).
//! 3. Every `match` block passes `colab_dsl::check` (what the editor
//!    diagnoses with) and lowers to the same rule `compile` produces.
//!
//! Every case also pins down what the agent-facing surfaces report:
//!
//! - `rules.expected` — per-rule attribution after one pass: how many
//!   files each rule changed, and for a dead rule the near-miss values
//!   `colab_core::near_miss::probe` would suggest.
//! - `find.expected` — what `colab find` reports over `input/`: every
//!   `path:line:col`, the rule that matched, the matched text, and the
//!   metavariable bindings of `<lang>::pattern` rules.
//!
//! Run with `COLAB_BLESS=1` to (re)write both files from the current
//! behaviour; review the diff before committing it.
//!
//! Adding a new language backend is gated on landing at least one case.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use colab_core::find::locate_all;
use colab_core::near_miss;
use colab_core::{BackendRegistry, CodeTransformer, FileChange, RunReport};
use colab_dsl::{Refactoring, check, compile_at_path};

fn registry() -> BackendRegistry {
    // Shared with the binary, so a backend cannot be registered in one
    // and missing from the other.
    colab_backends::registry()
}

/// Resolve `tests/corpus/` at the workspace root from the dsl crate's
/// manifest dir.
fn corpus_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .expect("workspace root above crates/colab-dsl")
        .join("tests")
        .join("corpus")
}

#[test]
fn corpus_cases_pass() {
    let root = corpus_root();
    assert!(root.is_dir(), "corpus root missing: {}", root.display());

    let cases = collect_cases(&root);
    assert!(
        !cases.is_empty(),
        "no corpus cases found under {}",
        root.display()
    );

    let mut failures: Vec<String> = Vec::new();
    for case in &cases {
        if let Err(err) = run_case(case) {
            failures.push(format!("{}: {}", case.name, err));
        }
    }

    assert!(
        failures.is_empty(),
        "corpus failures:\n  - {}",
        failures.join("\n  - ")
    );
}

struct Case {
    name: String,
    dir: PathBuf,
}

fn collect_cases(root: &Path) -> Vec<Case> {
    let mut out = Vec::new();
    let langs = fs::read_dir(root).expect("read corpus root");
    for lang_entry in langs.flatten() {
        let lang_path = lang_entry.path();
        if !lang_path.is_dir() {
            continue;
        }
        let lang = lang_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let cases = fs::read_dir(&lang_path).expect("read lang dir");
        for case_entry in cases.flatten() {
            let case_path = case_entry.path();
            if !case_path.is_dir() {
                continue;
            }
            let case_name = case_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            out.push(Case {
                name: format!("{}/{}", lang, case_name),
                dir: case_path,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn run_case(case: &Case) -> Result<(), String> {
    let script_path = case.dir.join("script");
    let refactoring =
        compile_at_path(&script_path, &registry()).map_err(|e| format!("compile: {}", e))?;

    let input_dir = case.dir.join("input");
    let expected_dir = case.dir.join("expected");

    let inputs = collect_files(&input_dir).map_err(|e| format!("walk input/: {}", e))?;
    let expected = collect_files(&expected_dir).map_err(|e| format!("walk expected/: {}", e))?;

    if inputs.keys().collect::<Vec<_>>() != expected.keys().collect::<Vec<_>>() {
        return Err(format!(
            "input and expected file sets differ\n  input: {:?}\n  expected: {:?}",
            inputs.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>()
        ));
    }

    check_matches_compile(&script_path, &refactoring)?;

    let mut report = RunReport::with_rules(refactoring.rules.iter().map(|r| r.to_string()));

    for (rel, source) in &inputs {
        let want = expected
            .get(rel)
            .ok_or_else(|| format!("missing expected file {}", rel.display()))?;

        // `apply_at` is what the walker calls, so the corpus exercises
        // per-rule path gating, the literal prefilter, and rule
        // attribution — not just the path-blind fallback.
        let outcome = refactoring.apply_at(rel, source);
        if refactoring.is_file_relevant(rel) {
            report.record(&FileChange {
                path: rel.clone(),
                before: source.clone(),
                after: outcome.output.clone(),
                rules_fired: outcome.rules_fired,
                conflicts: outcome.conflicts,
            });
        }
        let first = outcome.output;

        if &first != want {
            return Err(format!(
                "{}: output != expected\n--- expected\n{}\n--- got\n{}",
                rel.display(),
                want,
                first
            ));
        }

        // Idempotency: a second application must not change the output.
        // This is also the guard against an over-narrow prefilter — a
        // rule that skipped work on pass one but not pass two shows up
        // here.
        let second = refactoring.apply_at(rel, &first).output;
        if second != first {
            return Err(format!(
                "{}: transform is not idempotent on its own output",
                rel.display()
            ));
        }
    }

    near_miss::probe(&refactoring.rules, &mut report, |path| {
        inputs.get(path).cloned()
    });
    expect_file(&case.dir.join("rules.expected"), &render_rules(&report))?;
    expect_file(
        &case.dir.join("find.expected"),
        &render_find(&refactoring, &inputs),
    )?;

    Ok(())
}

/// The LSP lowers each `match` block on its own (`check`); a script the
/// corpus can run must come out clean there too, rule for rule. Rules
/// pulled in by `include` are not visible to `check`, so only a script
/// without includes is compared index by index.
fn check_matches_compile(script_path: &Path, refactoring: &Refactoring) -> Result<(), String> {
    let text = fs::read_to_string(script_path).map_err(|e| format!("read script: {}", e))?;
    let checks = check(&text, &registry()).map_err(|e| format!("check: {}", e))?;
    for rule in &checks {
        if let Err(e) = &rule.outcome {
            return Err(format!(
                "check rejects {} ({}) that compile accepted: {}",
                rule.namespace, rule.action, e
            ));
        }
    }
    if !text.lines().any(|l| l.trim_start().starts_with("include")) {
        let checked: Vec<&str> = checks
            .iter()
            .filter_map(|c| c.outcome.as_deref().ok())
            .collect();
        let compiled: Vec<String> = refactoring.rules.iter().map(|r| r.to_string()).collect();
        if checked != compiled {
            return Err(format!(
                "check and compile disagree\n  check:   {:?}\n  compile: {:?}",
                checked, compiled
            ));
        }
    }
    Ok(())
}

/// One line per rule: files changed, then the rule; a dead rule's near
/// misses and its reason for leaving matches unchanged follow, indented.
/// Then one line per conflict a rule reported.
fn render_rules(report: &RunReport) -> String {
    let mut out = String::new();
    for stat in &report.rules {
        out.push_str(&format!("{} {}\n", stat.files_matched, stat.rule));
        for near in &stat.near_misses {
            out.push_str(&format!(
                "    near {} ({} file(s))\n",
                near.value, near.files
            ));
        }
        if let Some(why) = &stat.unchanged {
            out.push_str(&format!(
                "    unchanged in {} file(s): {}\n",
                why.files, why.message
            ));
        }
    }
    for conflict in &report.conflicts {
        out.push_str(&format!(
            "conflict {} {}: {}\n",
            conflict.path.display(),
            conflict
                .rule
                .map_or("file".to_string(), |r| format!("rule {}", r + 1)),
            conflict.message
        ));
    }
    out
}

/// `path:line:col: [rule] text`, each binding indented below it. Newlines
/// in matched text are shown as `\n` so one location stays one line.
fn render_find(refactoring: &Refactoring, inputs: &BTreeMap<PathBuf, String>) -> String {
    let mut out = String::new();
    for (rel, source) in inputs {
        let mut hits = locate_all(&refactoring.rules, rel, source);
        hits.sort_by_key(|(index, l)| (l.start, *index));
        for (index, at) in hits {
            out.push_str(&format!(
                "{}:{}:{}: [{}] {}\n",
                rel.display(),
                at.line,
                at.column,
                index,
                at.text.replace('\n', "\\n")
            ));
            for (name, value) in &at.captures {
                out.push_str(&format!("    ${} = {}\n", name, value.replace('\n', "\\n")));
            }
        }
    }
    out
}

/// Compare `actual` against `path`; with `COLAB_BLESS` set, write it
/// instead. The file is required, so a new case cannot skip it.
fn expect_file(path: &Path, actual: &str) -> Result<(), String> {
    let name = path.file_name().unwrap().to_string_lossy();
    if std::env::var_os("COLAB_BLESS").is_some() {
        return fs::write(path, actual).map_err(|e| format!("bless {}: {}", name, e));
    }
    let want = fs::read_to_string(path)
        .map_err(|_| format!("missing {} (create it with COLAB_BLESS=1)", name))?;
    if want != actual {
        return Err(format!(
            "{} differs (re-run with COLAB_BLESS=1 to accept)\n--- expected\n{}--- got\n{}",
            name, want, actual
        ));
    }
    Ok(())
}

fn collect_files(root: &Path) -> Result<BTreeMap<PathBuf, String>, String> {
    let mut out = BTreeMap::new();
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    walk(root, root, &mut out)?;
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, String>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("read_dir {}: {}", dir.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("entry: {}", e))?;
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| format!("strip_prefix: {}", e))?
                .to_path_buf();
            let source =
                fs::read_to_string(&path).map_err(|e| format!("read {}: {}", path.display(), e))?;
            out.insert(rel, source);
        }
    }
    Ok(())
}
