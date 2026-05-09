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
//!
//! Adding a new language backend is gated on landing at least one case.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use colab_core::{BackendRegistry, CodeTransformer};
use colab_dsl::compile_at_path;

fn registry() -> BackendRegistry {
    let mut r = BackendRegistry::new();
    r.register(Box::new(colab_lang_go::GoBackend));
    r.register(Box::new(colab_lang_java::JavaBackend));
    r.register(Box::new(colab_lang_js::JsBackend));
    r.register(Box::new(colab_lang_python::PythonBackend));
    r.register(Box::new(colab_lang_rust::RustBackend));
    r
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
    let refactoring = compile_at_path(&script_path, &registry())
        .map_err(|e| format!("compile: {}", e))?;

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

    for (rel, source) in &inputs {
        let want = expected
            .get(rel)
            .ok_or_else(|| format!("missing expected file {}", rel.display()))?;

        let applies = refactoring.is_file_relevant(rel);
        let first = if applies {
            refactoring.apply(source)
        } else {
            source.clone()
        };

        if &first != want {
            return Err(format!(
                "{}: output != expected\n--- expected\n{}\n--- got\n{}",
                rel.display(),
                want,
                first
            ));
        }

        // Idempotency: a second application must not change the output.
        let second = if applies {
            refactoring.apply(&first)
        } else {
            first.clone()
        };
        if second != first {
            return Err(format!(
                "{}: transform is not idempotent on its own output",
                rel.display()
            ));
        }
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
