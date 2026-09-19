//! The shipped agent skill must only name namespaces the binary has.
//!
//! `plugins/claude/colab/skills/colab-codemod/` is distributed as a
//! Claude Code plugin, so it is read by an agent that cannot check its
//! claims — a `<lang>::<module>` pair that does not exist sends the
//! agent after a rule the compiler will reject (exit 3), and it has no
//! way to tell a stale doc from its own mistake. This is exactly the
//! drift `docs_match_capabilities.rs` catches for `docs/features.md`;
//! the skill earns the same guard because it ages the same way. Writing
//! it caught a leftover "js::import has no ensure" claim that had been
//! wrong since the action was added.
//!
//! The registry is the source of truth; the skill must match it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Namespace pairs that appear deliberately misspelled, as the worked
/// example of what colab's error message looks like. Each must stay
/// unknown to the registry — the test checks that too, so a module that
/// later acquires this name cannot sit here unnoticed.
const DELIBERATE_TYPOS: [&str; 1] = ["rust::symbl"];

fn skill_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root above crates/colab-backends")
        .join("plugins/claude/colab/skills/colab-codemod")
}

fn skill_files() -> Vec<(PathBuf, String)> {
    let dir = skill_dir();
    [
        "SKILL.md",
        "references/recipes.md",
        "references/troubleshooting.md",
    ]
    .iter()
    .map(|name| {
        let path = dir.join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        (path, text)
    })
    .collect()
}

/// `(lang, module) -> {actions}` as the registry reports it.
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

fn is_ident(c: char) -> bool {
    c.is_ascii_lowercase() || c == '_'
}

/// Every `lang::module` in the text whose `lang` is a registered
/// backend, with the 1-based line it sits on.
///
/// Deliberately narrow: the left and right sides must be
/// `[a-z_]`-only, and neither may be part of a longer path. That skips
/// the prose placeholders (`<lang>::symbol`), the language-level
/// wildcards (`js::*`), and the sample source that is not a colab
/// namespace at all (`use tokio::sync::Mutex`, PHP's `Old::run`).
fn namespaces_in(text: &str, languages: &BTreeSet<String>) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    for (lineno, line) in text.lines().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        for i in 0..chars.len().saturating_sub(1) {
            if chars[i] != ':' || chars[i + 1] != ':' {
                continue;
            }
            // Walk left over the language name.
            let mut start = i;
            while start > 0 && is_ident(chars[start - 1]) {
                start -= 1;
            }
            // Walk right over the module name.
            let mut end = i + 2;
            while end < chars.len() && is_ident(chars[end]) {
                end += 1;
            }
            if start == i || end == i + 2 {
                continue; // one side empty: `::foo`, `bar::`, `a::::b`
            }
            // Reject a segment of a longer path (`tokio::sync::Mutex`)
            // or a name we clipped mid-word (`Old::run`).
            let before = start.checked_sub(1).map(|j| chars[j]);
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':' || c == '.')
            {
                continue;
            }
            if chars
                .get(end)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
            {
                continue;
            }
            let lang: String = chars[start..i].iter().collect();
            let module: String = chars[i + 2..end].iter().collect();
            if languages.contains(&lang) {
                out.push((lang, module, lineno + 1));
            }
        }
    }
    out
}

#[test]
fn every_namespace_the_skill_names_exists() {
    let actual = actual_matrix();
    let languages: BTreeSet<String> = actual.keys().map(|(l, _)| l.clone()).collect();
    let typos: BTreeSet<&str> = DELIBERATE_TYPOS.into_iter().collect();

    let mut found = 0usize;
    let mut problems = Vec::new();

    for (path, text) in skill_files() {
        for (lang, module, line) in namespaces_in(&text, &languages) {
            let pair = format!("{lang}::{module}");
            if typos.contains(pair.as_str()) {
                continue;
            }
            found += 1;
            if !actual.contains_key(&(lang.clone(), module.clone())) {
                let known: Vec<&str> = actual
                    .keys()
                    .filter(|(l, _)| *l == lang)
                    .map(|(_, m)| m.as_str())
                    .collect();
                problems.push(format!(
                    "{}:{line}: `{pair}` does not exist; `{lang}` has: {}",
                    path.file_name().expect("file").to_string_lossy(),
                    known.join(", ")
                ));
            }
        }
    }

    assert!(
        found > 40,
        "only {found} namespace references parsed out of the skill — \
         has its format changed? This test would then be near-vacuous."
    );
    assert!(
        problems.is_empty(),
        "the colab-codemod skill names namespaces the registry does not \
         provide (the registry is the source of truth — fix the skill):\n  - {}",
        problems.join("\n  - ")
    );
}

#[test]
fn the_deliberate_typos_are_still_typos() {
    let actual = actual_matrix();
    for pair in DELIBERATE_TYPOS {
        let (lang, module) = pair.split_once("::").expect("lang::module");
        assert!(
            !actual.contains_key(&(lang.to_string(), module.to_string())),
            "`{pair}` is allowlisted as a deliberate typo in the skill, \
             but the registry now provides it — drop it from \
             DELIBERATE_TYPOS and fix the skill's error-message example"
        );
    }
}

#[test]
fn the_skill_names_every_backend() {
    // Guards against a backend being added and the skill's list of
    // namespaces quietly staying at the old count.
    let registry = colab_backends::registry();
    let (_, skill) = skill_files().swap_remove(0); // SKILL.md
    let missing: Vec<&str> = registry
        .languages()
        .into_iter()
        .filter(|lang| !skill.contains(&format!("`{lang}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "SKILL.md does not mention {} of {} registered backends: {missing:?}",
        missing.len(),
        colab_backends::BACKEND_COUNT
    );
}

#[test]
fn the_parser_can_actually_detect_a_bad_namespace() {
    // A guard that cannot fail is worthless. Feed the extractor a line
    // naming a module `go` does not have, and confirm it is reported.
    let languages: BTreeSet<String> = actual_matrix().keys().map(|(l, _)| l.clone()).collect();
    let found = namespaces_in("match go::improt \"a\" { replace \"b\" }", &languages);
    assert_eq!(
        found,
        vec![("go".to_string(), "improt".to_string(), 1)],
        "the extractor missed a namespace it must catch"
    );

    // And confirm the exclusions really exclude: none of these is a
    // colab namespace reference.
    for line in [
        "use tokio::sync::Mutex;",
        "match php::call \"Old::run\" { replace_call \"New::run($args)\" }",
        "`js::*` covers .js .mjs",
        "`<lang>::symbol` matches identifier text",
    ] {
        let hits = namespaces_in(line, &languages);
        let unexpected: Vec<_> = hits
            .iter()
            .filter(|(l, m, _)| !(l == "php" && m == "call"))
            .collect();
        assert!(
            unexpected.is_empty(),
            "extractor picked up non-namespace text in {line:?}: {unexpected:?}"
        );
    }
}
