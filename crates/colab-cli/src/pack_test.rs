//! `colab test`: a repository's own packs, checked against fixtures.
//!
//! A pack `house-style.colab` keeps its cases beside it, one directory
//! each:
//!
//! ```text
//! .colab/packs/house-style.colab
//! .colab/packs/house-style.tests/
//!     renames-the-import/input/main.go
//!     renames-the-import/expected/main.go     # what the pack makes of it
//!     leaves-vendored-code/input/vendor/x.go  # no expected/: must not change
//! ```
//!
//! Each case runs the pack over its `input/` files in memory, at their
//! paths relative to `input/` (so `in "<glob>"` scopes apply). It passes
//! when every file comes out as in `expected/` (a file `expected/` lacks
//! must come out unchanged, and a case with no `expected/` must change
//! nothing), a second run changes nothing (the idempotency check the
//! corpus harness makes), and no rule reports a conflict. A rule that
//! fires in no case is reported as untested. `--bless` (or
//! `COLAB_BLESS=1`) writes `expected/` from what the pack produces.
use std::fmt::Write as _;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use colab_core::{BackendRegistry, CodeTransformer, Error, Result};
use serde_json::{Value, json};

/// Exit code when a case fails.
pub const FAILED: u8 = 12;

#[derive(Parser, Debug)]
pub struct TestArgs {
    /// Packs (`.colab` files) or directories to search for packs with a
    /// `<pack>.tests/` directory. Default: the `path` packs `colab.toml`
    /// declares, and `.colab/packs/`.
    paths: Vec<PathBuf>,

    /// Write each case's `expected/` from what the pack produces (also
    /// `COLAB_BLESS=1`). Review the diff before committing it.
    #[arg(long)]
    bless: bool,

    /// `human`: a line per case, with diffs for failures. `json`: one
    /// document.
    #[arg(long, value_enum, default_value_t = TestFormat::Human)]
    format: TestFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum TestFormat {
    Human,
    Json,
}

/// What went wrong with one file of a case.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Failure {
    path: String,
    problem: String,
    diff: Option<String>,
}

#[derive(Debug)]
struct CaseResult {
    name: String,
    failures: Vec<Failure>,
    blessed: bool,
}

#[derive(Debug)]
struct PackResult {
    pack: PathBuf,
    cases: Vec<CaseResult>,
    /// Rules (index, description) that fired in no case.
    untested: Vec<(usize, String)>,
}

pub fn run(args: TestArgs, backends: &BackendRegistry) -> Result<u8> {
    let bless = args.bless || std::env::var("COLAB_BLESS").is_ok_and(|v| v == "1");
    let packs = discover(&args.paths)?;
    if packs.is_empty() {
        return Err(Error::Config(
            "no pack with tests found: put cases in `<pack>.tests/<case>/input/` beside a \
         `.colab` file (and `expected/` beside `input/`), or name the packs to test"
                .into(),
        ));
    }
    let results = packs
        .iter()
        .map(|pack| test_pack(pack, backends, bless))
        .collect::<Result<Vec<_>>>()?;
    let failed = results
        .iter()
        .flat_map(|p| &p.cases)
        .filter(|c| !c.failures.is_empty())
        .count();
    let mut out = io::stdout().lock();
    let written = match args.format {
        TestFormat::Json => {
            let doc = to_json(&results);
            writeln!(out, "{doc}")
        }
        TestFormat::Human => write!(out, "{}", to_human(&results)),
    };
    written.map_err(crate::cli::io_to_error)?;
    Ok(if failed > 0 { FAILED } else { 0 })
}

/// The packs to test: `.colab` files named, or found (with a tests
/// directory) under the directories named or the defaults.
fn discover(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut roots: Vec<PathBuf> = paths.to_vec();
    if roots.is_empty() {
        if let Some(config) = crate::config::find()? {
            for pack in &config.packs {
                match pack {
                    crate::config::PackRef::Path(p) => roots.push(config.dir.join(p)),
                    crate::config::PackRef::Git { url, pin, path } => {
                        let pack = crate::git_packs::checkout(url, pin)?.join(path);
                        if tests_dir(&pack).is_dir() {
                            roots.push(pack);
                        }
                    }
                    crate::config::PackRef::Builtin(_) => {}
                }
            }
        }
        roots.extend(crate::packs::repo_packs_dir());
    }
    let mut out = BTreeSet::new();
    for root in roots {
        if root.is_file() {
            if tests_dir(&root).is_dir() {
                out.insert(root);
            } else if !paths.is_empty() {
                return Err(Error::Config(format!(
                    "{} has no tests: add cases under {}/<case>/input/",
                    root.display(),
                    tests_dir(&root).display()
                )));
            }
        } else if root.is_dir() {
            find_packs(&root, &mut out);
        } else {
            return Err(Error::Config(format!(
                "{}: no such pack or directory",
                root.display()
            )));
        }
    }
    Ok(out.into_iter().collect())
}

fn find_packs(dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // A pack's own fixtures are never packs.
            if !path.extension().is_some_and(|e| e == "tests") {
                find_packs(&path, out);
            }
        } else if colab_core::script_file::is_script(&path) && tests_dir(&path).is_dir() {
            out.insert(path);
        }
    }
}

/// `<dir>/<stem>.tests` for `<dir>/<stem>.colab`.
fn tests_dir(pack: &Path) -> PathBuf {
    pack.with_extension("tests")
}

/// Each failing case of `pack`, as `<case>: <path>: <problem>`; empty
/// when they all pass.
pub fn failing_cases(pack: &Path, backends: &BackendRegistry) -> Result<Vec<String>> {
    let result = test_pack(pack, backends, false)?;
    Ok(result
        .cases
        .iter()
        .flat_map(|c| {
            c.failures.iter().map(move |f| {
                if f.path.is_empty() {
                    format!("{}: {}", c.name, f.problem)
                } else {
                    format!("{}: {}: {}", c.name, f.path, f.problem)
                }
            })
        })
        .collect())
}

fn test_pack(pack: &Path, backends: &BackendRegistry, bless: bool) -> Result<PackResult> {
    let refactoring = colab_dsl::compile_at_path(pack, backends)?;
    let mut cases = Vec::new();
    let mut fired = BTreeSet::new();
    let mut entries: Vec<PathBuf> = fs::read_dir(tests_dir(pack))
        .map_err(|e| Error::io_at(tests_dir(pack), e))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    for case in entries {
        let name = case
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let input = case.join("input");
        if !input.is_dir() {
            cases.push(CaseResult {
                name,
                failures: vec![Failure {
                    path: String::new(),
                    problem: "the case has no `input/` directory".into(),
                    diff: None,
                }],
                blessed: false,
            });
            continue;
        }
        let result = run_case(&refactoring, &case, &input, bless, &mut fired)?;
        cases.push(CaseResult { name, ..result });
    }
    let untested = (0..refactoring.len())
        .filter(|i| !fired.contains(i))
        .map(|i| (i + 1, refactoring.rules[i].to_string()))
        .collect();
    // Shown relative to the working directory when it is under it.
    let shown = std::env::current_dir()
        .ok()
        .and_then(|cwd| pack.strip_prefix(&cwd).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| pack.to_path_buf());
    Ok(PackResult {
        pack: shown,
        cases,
        untested,
    })
}

fn run_case(
    refactoring: &colab_dsl::Refactoring,
    case: &Path,
    input: &Path,
    bless: bool,
    fired: &mut BTreeSet<usize>,
) -> Result<CaseResult> {
    let expected_dir = case.join("expected");
    let inputs = read_tree(input)?;
    let expected = if expected_dir.is_dir() {
        read_tree(&expected_dir)?
    } else {
        BTreeMap::new()
    };
    let mut failures = Vec::new();
    let mut outputs = BTreeMap::new();
    for (rel, before) in &inputs {
        let path = Path::new(rel);
        let outcome = refactoring.apply_at(path, before);
        fired.extend(outcome.rules_fired.iter().copied());
        for conflict in &outcome.conflicts {
            failures.push(Failure {
                path: rel.clone(),
                problem: format!(
                    "rule {} reports a conflict: {}",
                    conflict.rule + 1,
                    conflict.message
                ),
                diff: None,
            });
        }
        let again = refactoring.apply_at(path, &outcome.output).output;
        if again != outcome.output {
            failures.push(Failure {
                path: rel.clone(),
                problem: "a second run changes it again: the pack is not idempotent".into(),
                diff: Some(colab_core::render::unified_diff(
                    path,
                    &outcome.output,
                    &again,
                )),
            });
        }
        outputs.insert(rel.clone(), outcome.output);
    }
    for rel in expected.keys() {
        if !inputs.contains_key(rel) {
            failures.push(Failure {
                path: rel.clone(),
                problem: "`expected/` has a file `input/` does not".into(),
                diff: None,
            });
        }
    }
    let changed: BTreeMap<&String, &String> = outputs
        .iter()
        .filter(|(rel, out)| inputs.get(*rel) != Some(*out))
        .collect();
    if bless {
        // The pack's own failures (conflicts, a second run) stand; the
        // expectations are rewritten.
        if expected_dir.is_dir() {
            fs::remove_dir_all(&expected_dir).map_err(|e| Error::io_at(&expected_dir, e))?;
        }
        for (rel, out) in &changed {
            let path = expected_dir.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
            }
            fs::write(&path, out).map_err(|e| Error::io_at(&path, e))?;
        }
        return Ok(CaseResult {
            name: String::new(),
            failures,
            blessed: true,
        });
    }
    for (rel, out) in &outputs {
        let want = expected.get(rel).unwrap_or(&inputs[rel]);
        if out != want {
            let problem = if expected.contains_key(rel) {
                "comes out different from `expected/`"
            } else if expected_dir.is_dir() {
                "changes, but `expected/` has no copy of it (so it must not change)"
            } else {
                "changes, but the case has no `expected/` (so nothing may change)"
            };
            failures.push(Failure {
                path: rel.clone(),
                problem: problem.into(),
                diff: Some(colab_core::render::unified_diff(Path::new(rel), want, out)),
            });
        }
    }
    Ok(CaseResult {
        name: String::new(),
        failures,
        blessed: false,
    })
}

/// Every file under `root`, by `/`-separated path relative to it.
fn read_tree(root: &Path) -> Result<BTreeMap<String, String>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
        for entry in fs::read_dir(dir)
            .map_err(|e| Error::io_at(dir, e))?
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else {
                let text = fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))?;
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(rel, text);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out)?;
    Ok(out)
}

fn to_json(results: &[PackResult]) -> Value {
    let (mut passed, mut failed) = (0, 0);
    let packs: Vec<Value> = results
        .iter()
        .map(|p| {
            let cases: Vec<Value> = p
                .cases
                .iter()
                .map(|c| {
                    if c.failures.is_empty() {
                        passed += 1;
                    } else {
                        failed += 1;
                    }
                    let failures: Vec<Value> = c
                        .failures
                        .iter()
                        .map(|f| json!({"path": f.path, "problem": f.problem, "diff": f.diff}))
                        .collect();
                    json!({
                        "name": c.name,
                        "passed": c.failures.is_empty(),
                        "blessed": c.blessed,
                        "failures": failures,
                    })
                })
                .collect();
            let untested: Vec<Value> = p
                .untested
                .iter()
                .map(|(rule, d)| json!({"rule": rule, "description": d}))
                .collect();
            json!({
                "pack": p.pack.to_string_lossy(),
                "cases": cases,
                "untested_rules": untested,
            })
        })
        .collect();
    json!({"packs": packs, "passed": passed, "failed": failed})
}

fn to_human(results: &[PackResult]) -> String {
    let mut out = String::new();
    let (mut passed, mut failed) = (0, 0);
    for p in results {
        let _ = writeln!(out, "{}", p.pack.display());
        for c in &p.cases {
            if c.failures.is_empty() {
                passed += 1;
                let mark = if c.blessed { "blessed" } else { "ok" };
                let _ = writeln!(out, "  {mark}   {}", c.name);
                continue;
            }
            failed += 1;
            let _ = writeln!(out, "  FAIL {}", c.name);
            for f in &c.failures {
                let at = if f.path.is_empty() {
                    String::new()
                } else {
                    format!("{}: ", f.path)
                };
                let _ = writeln!(out, "       {at}{}", f.problem);
                if let Some(diff) = &f.diff {
                    for line in diff.lines() {
                        let _ = writeln!(out, "         {line}");
                    }
                }
            }
        }
        for (rule, description) in &p.untested {
            let _ = writeln!(
                out,
                "  warning: rule {rule} fires in no case: {description}"
            );
        }
    }
    let _ = writeln!(out, "{passed} passed, {failed} failed");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "colab-pack-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    const PACK: &str = r#"refactor "house" {
    match go::import "old/log" in "src/**" { replace "new/log" }
    match go::import "never/seen" { replace "x" }
}"#;
    const GO: &str = "package p\n\nimport \"old/log\"\n";

    fn pack(dir: &Path) -> PathBuf {
        let pack = dir.join("house.colab");
        write(&pack, PACK);
        write(&dir.join("house.tests/renames/input/src/a.go"), GO);
        write(
            &dir.join("house.tests/renames/expected/src/a.go"),
            &GO.replace("old/log", "new/log"),
        );
        // Outside `src/`: must not change.
        write(&dir.join("house.tests/scoped/input/vendor/a.go"), GO);
        pack
    }

    #[test]
    fn cases_pass_and_untested_rules_are_named() {
        let dir = temp_dir("pass");
        let pack = pack(&dir);
        let result = test_pack(&pack, &colab_backends::registry(), false).unwrap();
        let names: Vec<_> = result.cases.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["renames", "scoped"]);
        assert!(
            result.cases.iter().all(|c| c.failures.is_empty()),
            "{result:?}"
        );
        assert_eq!(result.untested.len(), 1);
        assert_eq!(result.untested[0].0, 2);
        assert_eq!(discover(std::slice::from_ref(&dir)).unwrap(), [pack]);
    }

    #[test]
    fn a_wrong_expectation_fails_with_a_diff_and_bless_rewrites_it() {
        let dir = temp_dir("fail");
        let pack = pack(&dir);
        write(&dir.join("house.tests/renames/expected/src/a.go"), GO);
        // A case with no `expected/` that the pack changes.
        write(&dir.join("house.tests/changes/input/src/b.go"), GO);
        let registry = colab_backends::registry();
        let result = test_pack(&pack, &registry, false).unwrap();
        let failing: Vec<_> = result
            .cases
            .iter()
            .filter(|c| !c.failures.is_empty())
            .map(|c| (c.name.as_str(), c.failures[0].problem.as_str()))
            .collect();
        assert_eq!(
            failing,
            [
                (
                    "changes",
                    "changes, but the case has no `expected/` (so nothing may change)"
                ),
                ("renames", "comes out different from `expected/`"),
            ]
        );
        assert!(
            result.cases[1].failures[0]
                .diff
                .as_ref()
                .unwrap()
                .contains("+import \"new/log\"")
        );
        let blessed = test_pack(&pack, &registry, true).unwrap();
        assert!(
            blessed
                .cases
                .iter()
                .all(|c| c.blessed && c.failures.is_empty())
        );
        let again = test_pack(&pack, &registry, false).unwrap();
        assert!(
            again.cases.iter().all(|c| c.failures.is_empty()),
            "{again:?}"
        );
        // The unchanged case got no `expected/`.
        assert!(!dir.join("house.tests/scoped/expected").exists());
    }

    #[test]
    fn a_pack_that_rewrites_its_own_output_fails() {
        let dir = temp_dir("loop");
        let pack = dir.join("wrap.colab");
        write(
            &pack,
            r#"refactor "w" { match go::call "f" { replace_call "f(ctx, $args)" } }"#,
        );
        write(
            &dir.join("wrap.tests/once/input/a.go"),
            "package p\n\nfunc m() { f(1) }\n",
        );
        let result = test_pack(&pack, &colab_backends::registry(), true).unwrap();
        assert!(
            result.cases[0].failures[0]
                .problem
                .contains("not idempotent")
        );
        let named = discover(&[dir.join("missing.colab")])
            .unwrap_err()
            .to_string();
        assert!(named.contains("no such pack"), "{named}");
    }
}
