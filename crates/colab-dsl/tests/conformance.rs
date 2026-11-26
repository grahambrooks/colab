//! The conformance matrix: every feature, tried in every language.
//!
//! Each `tests/conformance/<feature>.toml` names a feature and holds one
//! case per language:
//!
//! ```toml
//! feature = "Modifiers left out of a pattern"
//! description = "An item pattern that spells no modifiers matches an item with any, and a rewrite keeps them."
//!
//! [[case]]
//! lang = "java"
//! script = "match java::pattern 'void $F() {}' { replace 'void $F() { run(); }' }"
//! input = "class A {\n  public static void f() {}\n}\n"
//! expected = "class A {\n  public static void f() { run(); }\n}\n"
//!
//! [[case]]
//! lang = "c"
//! na = "C has no item modifiers a pattern could leave out."
//!
//! [[case]]
//! lang = "cpp"
//! xfail = "…why it does not work yet…"
//! script = "…"
//! input = "…"
//! expected = "…"
//! ```
//!
//! A generalize case has `before` and `after` instead of `script`: one
//! hand-made edit. The language's `LanguageBackend::generalize` learns
//! rules from it, and each is applied in turn as a `pattern` `replace` to
//! `input`, which must give `expected`:
//!
//! ```toml
//! [[case]]
//! lang = "go"
//! before = "assert.Equal(t, want, got)"
//! after = "require.Equal(t, want, got)"
//! input = "…another call of assert.Equal…"
//! expected = "…the same call of require.Equal…"
//! ```
//!
//! A case passes when the script turns `input` into `expected` and a
//! second run changes nothing. An `xfail` case is a known gap: it must
//! still fail (a fix that makes it pass fails the test until the `xfail`
//! is removed), so the matrix never claims less than colab does. `na` says
//! the feature does not apply.
//!
//! The run writes `docs/conformance.md`, a feature × language table, and
//! fails when the checked-in one differs; `COLAB_BLESS=1` rewrites it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use colab_core::CodeTransformer;
use toml_edit::{DocumentMut, Item, Table};

/// The languages the matrix has a column for, with a file name a case's
/// input is taken to have (its extension picks the backend's files).
const LANGUAGES: &[(&str, &str)] = &[
    ("c", "sample.c"),
    ("cpp", "sample.cpp"),
    ("csharp", "Sample.cs"),
    ("go", "sample.go"),
    ("java", "Sample.java"),
    ("js", "sample.js"),
    ("kotlin", "Sample.kt"),
    ("php", "sample.php"),
    ("python", "sample.py"),
    ("rust", "sample.rs"),
    ("swift", "Sample.swift"),
    ("ts", "sample.ts"),
    ("tsx", "sample.tsx"),
];

/// What one cell of the matrix says.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Cell {
    Pass,
    /// A known gap, with why.
    KnownGap(String),
    /// The feature does not apply, with why.
    NotApplicable(String),
    /// No case written yet.
    Missing,
}

struct Feature {
    file: String,
    name: String,
    description: String,
    cells: BTreeMap<String, Cell>,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn string(table: &Table, key: &str) -> Option<String> {
    table.get(key).and_then(Item::as_str).map(str::to_string)
}

/// Run one case: `Ok(())` when it does what it says, else why not.
fn run_case(lang: &str, case: &Table) -> Result<(), String> {
    let input = string(case, "input").ok_or("no `input`")?;
    let expected = string(case, "expected").ok_or("no `expected`")?;
    if let Some(before) = string(case, "before") {
        let after = string(case, "after").ok_or("`before` with no `after`")?;
        return run_generalized(lang, &before, &after, &input, &expected);
    }
    let script = string(case, "script").ok_or("no `script` (nor `before`/`after`)")?;
    let path = string(case, "path").map_or_else(
        || {
            LANGUAGES
                .iter()
                .find(|(l, _)| *l == lang)
                .map_or_else(|| PathBuf::from("sample.txt"), |(_, p)| PathBuf::from(p))
        },
        PathBuf::from,
    );
    let backends = colab_backends::registry();
    let refactoring =
        colab_dsl::compile(&script, &backends).map_err(|e| format!("compile: {e}"))?;
    let first = refactoring.apply_at(&path, &input).output;
    if first != expected {
        return Err(format!(
            "output differs\n--- expected\n{expected}--- got\n{first}"
        ));
    }
    let second = refactoring.apply_at(&path, &first).output;
    if second != first {
        return Err(format!("not idempotent: a second run gives\n{second}"));
    }
    Ok(())
}

/// A generalize case: the rules `lang`'s backend learns from the edit
/// `before` → `after`, each applied as a `pattern` `replace` in turn, must
/// turn `input` into `expected`, and change nothing on a second pass.
fn run_generalized(
    lang: &str,
    before: &str,
    after: &str,
    input: &str,
    expected: &str,
) -> Result<(), String> {
    let registry = colab_backends::registry();
    let backend = registry
        .get(lang)
        .ok_or_else(|| format!("no backend `{lang}`"))?;
    let rules = backend
        .generalize(before, after)
        .map_err(|e| format!("generalize: {e}"))?;
    if rules.is_empty() {
        return Err("generalize learned no rule".into());
    }
    let mut ops = Vec::new();
    for rule in &rules {
        let op = backend
            .build_rule(
                "pattern",
                colab_core::RuleSpec::Replace {
                    target: rule.pattern.clone(),
                    replacement: rule.template.clone(),
                },
            )
            .map_err(|e| format!("rule `{}` → `{}`: {e}", rule.pattern, rule.template))?;
        ops.push(op);
    }
    let learned = || {
        rules
            .iter()
            .map(|r| format!("`{}` → `{}`", r.pattern, r.template))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let run = |text: &str| ops.iter().fold(text.to_string(), |t, op| op.apply(&t));
    let first = run(input);
    if first != expected {
        return Err(format!(
            "learned {}; output differs\n--- expected\n{expected}--- got\n{first}",
            learned()
        ));
    }
    let second = run(&first);
    if second != first {
        return Err(format!(
            "learned {}; not idempotent: a second run gives\n{second}",
            learned()
        ));
    }
    Ok(())
}

fn load(path: &Path, failures: &mut Vec<String>) -> Feature {
    let file = path.file_name().unwrap().to_string_lossy().into_owned();
    let text = fs::read_to_string(path).unwrap();
    let doc: DocumentMut = text
        .parse()
        .unwrap_or_else(|e| panic!("{file}: not TOML: {e}"));
    let top = doc.as_table();
    let name = string(top, "feature").unwrap_or_else(|| panic!("{file}: no `feature`"));
    let description = string(top, "description").unwrap_or_default();
    let mut cells: BTreeMap<String, Cell> = LANGUAGES
        .iter()
        .map(|(l, _)| ((*l).to_string(), Cell::Missing))
        .collect();
    let cases = top
        .get("case")
        .and_then(Item::as_array_of_tables)
        .map_or_default(|a| a.iter().collect::<Vec<_>>());
    for case in cases {
        let Some(lang) = string(case, "lang") else {
            failures.push(format!("{file}: a case with no `lang`"));
            continue;
        };
        if !cells.contains_key(&lang) {
            failures.push(format!("{file}: unknown language `{lang}`"));
            continue;
        }
        if cells[&lang] != Cell::Missing {
            failures.push(format!("{file}: two cases for `{lang}`"));
        }
        let cell = if let Some(why) = string(case, "na") {
            Cell::NotApplicable(why)
        } else {
            let outcome = run_case(&lang, case);
            match (string(case, "xfail"), outcome) {
                (None, Ok(())) => Cell::Pass,
                (None, Err(why)) => {
                    failures.push(format!("{file} [{lang}]: {why}"));
                    Cell::Missing
                }
                (Some(gap), Err(_)) => Cell::KnownGap(gap),
                (Some(_), Ok(())) => {
                    failures.push(format!(
                        "{file} [{lang}]: marked `xfail` but passes now: remove the `xfail`"
                    ));
                    Cell::Pass
                }
            }
        };
        cells.insert(lang, cell);
    }
    Feature {
        file,
        name,
        description,
        cells,
    }
}

/// `docs/conformance.md` for `features`.
fn render(features: &[Feature]) -> String {
    let mut out = String::from(
        "# Conformance matrix\n\n\
         Generated by `cargo test -p colab-dsl --test conformance` from\n\
         [`tests/conformance/`](../tests/conformance/) — do not edit by hand;\n\
         `COLAB_BLESS=1` rewrites it. Each cell is a case that runs the feature\n\
         in that language and checks the output and a second, unchanged run.\n\n\
         ✅ works · ❌ a known gap (see below) · n/a does not apply · — no case yet\n\n",
    );
    out.push_str("| Feature |");
    for (lang, _) in LANGUAGES {
        let _ = write!(out, " {lang} |");
    }
    out.push_str("\n| --- |");
    for _ in LANGUAGES {
        out.push_str(" :-: |");
    }
    out.push('\n');
    let mut notes: Vec<String> = Vec::new();
    for f in features {
        let _ = write!(out, "| [{}](#{}) |", f.name, anchor(&f.name));
        for (lang, _) in LANGUAGES {
            let cell = match &f.cells[*lang] {
                Cell::Pass => "✅".to_string(),
                Cell::KnownGap(_) => "❌".to_string(),
                Cell::NotApplicable(_) => "n/a".to_string(),
                Cell::Missing => "—".to_string(),
            };
            let _ = write!(out, " {cell} |");
        }
        out.push('\n');
    }
    let (mut pass, mut gap, mut na, mut missing) = (0, 0, 0, 0);
    for f in features {
        for cell in f.cells.values() {
            match cell {
                Cell::Pass => pass += 1,
                Cell::KnownGap(_) => gap += 1,
                Cell::NotApplicable(_) => na += 1,
                Cell::Missing => missing += 1,
            }
        }
    }
    let _ = writeln!(
        out,
        "\n{pass} working, {gap} known gaps, {na} not applicable, {missing} without a case.\n"
    );
    for f in features {
        let _ = writeln!(out, "## {}\n\n{}\n", f.name, f.description);
        let _ = writeln!(
            out,
            "Cases: [`tests/conformance/{0}`](../tests/conformance/{0}).\n",
            f.file
        );
        notes.clear();
        for (lang, _) in LANGUAGES {
            match &f.cells[*lang] {
                Cell::KnownGap(why) => notes.push(format!("- ❌ **{lang}**: {why}")),
                Cell::NotApplicable(why) => notes.push(format!("- n/a **{lang}**: {why}")),
                _ => {}
            }
        }
        if !notes.is_empty() {
            out.push_str(&notes.join("\n"));
            out.push_str("\n\n");
        }
    }
    out
}

/// A heading's GitHub anchor.
fn anchor(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            'a'..='z' | '0'..='9' | '-' | '_' => Some(c),
            ' ' => Some('-'),
            _ => None,
        })
        .collect()
}

#[test]
fn every_feature_in_every_language() {
    let dir = repo_root().join("tests/conformance");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no feature files in {}", dir.display());
    let mut failures = Vec::new();
    let features: Vec<Feature> = files.iter().map(|p| load(p, &mut failures)).collect();
    let doc = render(&features);
    let path = repo_root().join("docs/conformance.md");
    if std::env::var_os("COLAB_BLESS").is_some() {
        fs::write(&path, &doc).unwrap();
    } else if fs::read_to_string(&path).ok().as_deref() != Some(doc.as_str()) {
        failures.push(
            "docs/conformance.md is out of date: run `COLAB_BLESS=1 cargo test -p colab-dsl --test conformance`"
                .into(),
        );
    }
    assert!(
        failures.is_empty(),
        "conformance failures:\n{}",
        failures.join("\n\n")
    );
}
