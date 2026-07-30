//! `docs/features.md` must agree with what the binary actually advertises.
//!
//! The capability matrix in that file is a 44-row hand-maintained replica
//! of `colab schema`. Nothing stopped the two drifting: when seven
//! languages were added the table was updated by hand and verified by a
//! one-off script, which is exactly the kind of check that stops being
//! run. A wrong row sends someone — or an agent reading the docs — after
//! a module or action that does not exist.
//!
//! This turns that from a habit into a build failure. The registry is the
//! source of truth; the docs must match it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Actions the matrix has a column for, in column order.
const COLUMNS: [&str; 4] = ["replace", "delete", "ensure", "replace_call"];

fn features_md() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root above crates/colab-backends")
        .join("docs")
        .join("features.md")
}

/// The `(lang, module) -> {actions}` map the registry reports.
fn actual_matrix() -> BTreeMap<(String, String), BTreeSet<String>> {
    let registry = colab_backends::registry();
    let mut out = BTreeMap::new();
    for lang in registry.languages() {
        let backend = registry.get(lang).expect("registry lists it");
        for capability in backend.capabilities() {
            out.insert(
                (lang.to_string(), capability.module.to_string()),
                capability
                    .actions
                    .iter()
                    .map(|a| a.name.to_string())
                    .collect(),
            );
        }
    }
    out
}

/// The same map, parsed out of the markdown table.
///
/// Rows look like:
/// `| `go::import` | ✅ | ✅ | ✅ |    | .go |`
fn documented_matrix(text: &str) -> BTreeMap<(String, String), BTreeSet<String>> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("| `") {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        // namespace + 4 action columns + at least one more (the files column)
        if cells.len() < 6 {
            continue;
        }
        let Some(ns) = cells[0].strip_prefix('`').and_then(|s| s.strip_suffix('`')) else {
            continue;
        };
        let Some((lang, module)) = ns.split_once("::") else {
            continue;
        };
        let actions: BTreeSet<String> = COLUMNS
            .iter()
            .enumerate()
            .filter(|(i, _)| cells[i + 1].contains('✅'))
            .map(|(_, name)| name.to_string())
            .collect();
        out.insert((lang.to_string(), module.to_string()), actions);
    }
    out
}

#[test]
fn features_md_matrix_matches_the_registry() {
    let text = std::fs::read_to_string(features_md()).expect("read docs/features.md");
    let documented = documented_matrix(&text);
    let actual = actual_matrix();

    assert!(
        !documented.is_empty(),
        "parsed no rows out of docs/features.md — has the table format changed? \
         This test would then be silently vacuous."
    );

    let mut problems = Vec::new();

    for (key, actions) in &actual {
        match documented.get(key) {
            None => problems.push(format!(
                "{}::{} exists but is missing from the matrix",
                key.0, key.1
            )),
            Some(doc) if doc != actions => problems.push(format!(
                "{}::{} documents {:?} but supports {:?}",
                key.0, key.1, doc, actions
            )),
            Some(_) => {}
        }
    }
    for key in documented.keys() {
        if !actual.contains_key(key) {
            problems.push(format!(
                "{}::{} is documented but no backend provides it",
                key.0, key.1
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "docs/features.md is out of step with the registry \
         (the registry is the source of truth — fix the docs):\n  - {}",
        problems.join("\n  - ")
    );
}

#[test]
fn the_matrix_covers_every_backend() {
    // Guards against a table that parses but has quietly lost a whole
    // language section.
    let text = std::fs::read_to_string(features_md()).expect("read docs/features.md");
    let documented = documented_matrix(&text);
    let langs: BTreeSet<&String> = documented.keys().map(|(l, _)| l).collect();
    assert_eq!(
        langs.len(),
        colab_backends::BACKEND_COUNT,
        "matrix documents {} languages but {} are registered: {:?}",
        langs.len(),
        colab_backends::BACKEND_COUNT,
        langs
    );
}

#[test]
fn the_parser_can_actually_detect_a_mismatch() {
    // A guard that cannot fail is worthless. Feed the parser a row that
    // claims an action `go::import` does not have, and confirm the
    // comparison notices.
    let fake = "| `go::import` | ✅ |    |    | ✅ | `.go` |\n";
    let documented = documented_matrix(fake);
    let actual = actual_matrix();
    let key = ("go".to_string(), "import".to_string());
    assert_ne!(
        documented.get(&key),
        actual.get(&key),
        "the parser read a deliberately wrong row as correct — \
         features_md_matrix_matches_the_registry would pass vacuously"
    );
}
