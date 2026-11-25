//! The two pattern engines side by side: every `<lang>::pattern` rule in
//! the conformance matrix (`tests/conformance/*.toml`) and the corpus
//! (`tests/corpus/<lang>/<case>/script`), read by the DSL's parser with
//! its `where` constraints, compiled by the ast-grep engine
//! (`SourceLanguage::compile_pattern`) and by colab's own
//! (`colab_rewrite::syntax::compile_forms` + `Checks`), and run over the
//! case's input. The matched ranges must agree, and a rule one engine
//! refuses the other must refuse too, except where an entry in `KNOWN`
//! says why they differ.
//!
//! This is the bar for moving `StructuralPattern` onto colab's matcher.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use colab_core::{Constraint, Operation};
use colab_dsl::ast::{Command, Item};
use colab_rewrite::pattern::PatternAction;

fn source_language(lang: &str) -> Option<&'static colab_rewrite::SourceLanguage> {
    Some(match lang {
        "c" => &colab_lang_c::LANG,
        "cpp" => &colab_lang_cpp::LANG,
        "csharp" => &colab_lang_csharp::LANG,
        "go" => &colab_lang_go::LANG,
        "java" => &colab_lang_java::LANG,
        "js" => &colab_lang_js::LANG,
        "kotlin" => &colab_lang_kotlin::LANG,
        "php" => &colab_lang_php::LANG,
        "python" => &colab_lang_python::LANG,
        "ruby" => &colab_lang_ruby::LANG,
        "rust" => &colab_lang_rust::LANG,
        "swift" => &colab_lang_swift::LANG,
        "ts" => &colab_lang_ts::LANG,
        "tsx" => &colab_lang_tsx::LANG,
        _ => return None,
    })
}

/// One `<lang>::pattern` rule: language, pattern, constraints.
type Rule = (String, String, Vec<Constraint>);

/// Every pattern rule in a script, `then` stages included.
fn rules(script: &str) -> Vec<Rule> {
    fn walk(items: &[Item], out: &mut Vec<Rule>) {
        for item in items {
            match item {
                Item::Match(m) if m.namespace.module == "pattern" => out.push((
                    m.namespace.lang.clone(),
                    m.match_string.clone(),
                    m.constraints.clone(),
                )),
                Item::Then(items) => walk(items, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    if let Ok(Command { items, .. }) = colab_dsl::parse(script) {
        walk(&items, &mut out);
    }
    out
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// (where, rule, input text) for every comparison.
fn cases(root: &Path) -> Vec<(String, Rule, String)> {
    let mut out = Vec::new();
    let mut tomls = Vec::new();
    files_under(&root.join("tests/conformance"), &mut tomls);
    tomls.sort();
    for path in tomls
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
    {
        let doc: toml_edit::DocumentMut = std::fs::read_to_string(path).unwrap().parse().unwrap();
        let Some(cases) = doc.get("case").and_then(|c| c.as_array_of_tables()) else {
            continue;
        };
        for (i, case) in cases.iter().enumerate() {
            let (Some(script), Some(input)) = (
                case.get("script").and_then(|s| s.as_str()),
                case.get("input").and_then(|s| s.as_str()),
            ) else {
                continue;
            };
            let name = path.file_name().unwrap().to_string_lossy();
            for rule in rules(script) {
                out.push((format!("{name}#{i}"), rule, input.to_string()));
            }
        }
    }
    let mut scripts = Vec::new();
    files_under(&root.join("tests/corpus"), &mut scripts);
    scripts.sort();
    for script in scripts
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "script"))
    {
        let case = script.parent().unwrap();
        let text = std::fs::read_to_string(script).unwrap();
        let mut inputs = Vec::new();
        files_under(&case.join("input"), &mut inputs);
        inputs.sort();
        for rule in rules(&text) {
            let Some(language) = source_language(&rule.0) else {
                continue;
            };
            for input in &inputs {
                let relevant = input
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| language.extensions.contains(&e));
                if let (true, Ok(source)) = (relevant, std::fs::read_to_string(input)) {
                    let shown = input.strip_prefix(root).unwrap_or(input).display();
                    out.push((shown.to_string(), rule.clone(), source));
                }
            }
        }
    }
    out
}

type Ranges = BTreeSet<(usize, usize)>;

/// A range with the whitespace at its end left out, as the old engine
/// reports a node that owns its newline (Go's `expression_case`).
fn trimmed(source: &str, start: usize, end: usize) -> (usize, usize) {
    (start, start + source[start..end].trim_end().len())
}

fn old_engine((lang, pattern, constraints): &Rule, source: &str) -> Result<Ranges, String> {
    let language = source_language(lang).unwrap();
    let op = language
        .compile_pattern(pattern, PatternAction::Delete, constraints)
        .map_err(|e| e.to_string())?;
    Ok(op
        .locate(source)
        .unwrap_or_default()
        .iter()
        .map(|l| trimmed(source, l.start, l.end))
        .collect())
}

fn new_engine((lang, pattern, constraints): &Rule, source: &str) -> Result<Ranges, String> {
    let language = (source_language(lang).unwrap().pattern)();
    let forms = colab_rewrite::syntax::compile_forms(&language, pattern)?;
    let bound = colab_rewrite::syntax::bound(&forms.forms[0].pattern);
    let checks = colab_rewrite::syntax::Checks::compile(&language, &bound, constraints)?;
    Ok(
        colab_rewrite::syntax::find_where(&language, &forms, &checks, source)
            .iter()
            .map(|f| trimmed(source, f.range.start, f.range.end))
            .collect(),
    )
}

/// Where the engines may differ, by the place's prefix, and why. Each is
/// work left before the switch; a known difference that stops differing
/// fails the test, so the list stays true.
const KNOWN: &[(&str, &str)] = &[];

#[test]
fn both_engines_find_the_same_matches() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut differ = Vec::new();
    let (mut compared, mut constrained, mut refused) = (0, 0, 0);
    for (place, rule, source) in cases(&root) {
        if source_language(&rule.0).is_none() {
            continue;
        }
        compared += 1;
        if !rule.2.is_empty() {
            constrained += 1;
        }
        // Scan facts and semantic answers come from a run; neither engine
        // has one here, so both read the same empty sets.
        let (old, new) = (old_engine(&rule, &source), new_engine(&rule, &source));
        let show = |r: &Ranges| -> Vec<String> {
            r.iter()
                .map(|&(s, e)| format!("{:?}", source[s..e].chars().take(50).collect::<String>()))
                .collect()
        };
        let line = match (&old, &new) {
            (Ok(a), Ok(b)) if a == b => continue,
            (Ok(a), Ok(b)) => format!(
                "old only {:?}; new only {:?}",
                show(&a.difference(b).copied().collect()),
                show(&b.difference(a).copied().collect())
            ),
            (Err(_), Err(_)) => {
                refused += 1;
                continue;
            }
            (Ok(_), Err(e)) => format!("only the new engine refuses it: {e}"),
            (Err(e), Ok(_)) => format!("only the old engine refuses it: {e}"),
        };
        let (lang, pattern, constraints) = &rule;
        let wheres: Vec<String> = constraints.iter().map(ToString::to_string).collect();
        differ.push((
            place,
            format!(
                "{lang}::pattern {pattern:?} where [{}]: {line}",
                wheres.join(", ")
            ),
        ));
    }
    eprintln!(
        "{compared} comparisons ({constrained} with `where`, {refused} refused by both), {} differ",
        differ.len()
    );
    // Both refusing a rule says nothing about matching: keep it rare.
    assert!(
        refused * 20 < compared,
        "{refused} of {compared} refused by both"
    );
    assert!(compared > 200, "too few comparisons: {compared}");
    let unexplained: Vec<String> = differ
        .iter()
        .filter(|(place, _)| !KNOWN.iter().any(|(prefix, _)| place.starts_with(prefix)))
        .map(|(place, line)| format!("{place} {line}"))
        .collect();
    assert!(
        unexplained.is_empty(),
        "the engines differ:\n{}",
        unexplained.join("\n")
    );
    for (prefix, why) in KNOWN {
        assert!(
            differ.iter().any(|(place, _)| place.starts_with(prefix)),
            "{prefix} no longer differs ({why}): take it out of KNOWN"
        );
    }
}
