//! The two pattern engines side by side: every `<lang>::pattern` in the
//! conformance matrix (`tests/conformance/*.toml`) and the corpus
//! (`tests/corpus/<lang>/<case>/script`), compiled by the ast-grep engine
//! (`SourceLanguage::compile_pattern`) and by colab's own
//! (`colab_rewrite::syntax::compile_forms`, every form), and run over the
//! case's input. The matched ranges must agree, except where an entry in
//! `KNOWN` says why they differ.
//!
//! This is the bar for moving `StructuralPattern` onto colab's matcher.
//! The pattern alone is compared: a rule's `where` narrows both alike.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use colab_core::Operation;
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

/// Each `match <lang>::pattern <string>` in a script: the language and
/// the pattern text, its quoting undone (`'…'` and raw strings literal,
/// `"…"` with JSON's escapes).
fn patterns(script: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = script;
    while let Some(at) = rest.find("::pattern") {
        let head = &rest[..at];
        let lang: String = head
            .chars()
            .rev()
            .take_while(char::is_ascii_lowercase)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let after = rest[at + "::pattern".len()..].trim_start();
        rest = &rest[at + "::pattern".len()..];
        let is_match = head[..head.len() - lang.len()]
            .trim_end()
            .ends_with("match");
        if !is_match || lang.is_empty() {
            continue;
        }
        let text = if let Some(body) = after.strip_prefix('\'') {
            body.find('\'').map(|end| body[..end].to_string())
        } else if let Some(body) = after.strip_prefix('"') {
            let mut text = String::new();
            let mut chars = body.chars();
            let mut closed = false;
            while let Some(c) = chars.next() {
                match c {
                    '"' => {
                        closed = true;
                        break;
                    }
                    '\\' => match chars.next() {
                        Some('n') => text.push('\n'),
                        Some('t') => text.push('\t'),
                        Some(c @ ('"' | '\\')) => text.push(c),
                        Some(c) => {
                            text.push('\\');
                            text.push(c);
                        }
                        None => {}
                    },
                    c => text.push(c),
                }
            }
            closed.then_some(text)
        } else if let Some(body) = after.strip_prefix("r#\"") {
            body.find("\"#").map(|end| body[..end].to_string())
        } else {
            None
        };
        if let Some(text) = text {
            out.push((lang, text));
        }
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

/// (where, language, pattern, input text) for every comparison.
fn cases(root: &Path) -> Vec<(String, String, String, String)> {
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
            for (lang, pattern) in patterns(script) {
                out.push((format!("{name}#{i}"), lang, pattern, input.to_string()));
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
        for (lang, pattern) in patterns(&text) {
            let Some(language) = source_language(&lang) else {
                continue;
            };
            for input in &inputs {
                let relevant = input
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| language.extensions.contains(&e));
                if let (true, Ok(source)) = (relevant, std::fs::read_to_string(input)) {
                    let shown = input.strip_prefix(root).unwrap_or(input).display();
                    out.push((shown.to_string(), lang.clone(), pattern.clone(), source));
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

fn old_engine(lang: &str, pattern: &str, source: &str) -> Result<Ranges, String> {
    let language = source_language(lang).unwrap();
    let op = language
        .compile_pattern(pattern, PatternAction::Delete, &[])
        .map_err(|e| e.to_string())?;
    Ok(op
        .locate(source)
        .unwrap_or_default()
        .iter()
        .map(|l| trimmed(source, l.start, l.end))
        .collect())
}

fn new_engine(lang: &str, pattern: &str, source: &str) -> Result<Ranges, String> {
    let language = (source_language(lang).unwrap().pattern)();
    let forms = colab_rewrite::syntax::compile_forms(&language, pattern)?;
    let tree = colab_rewrite::syntax::tree(&language.grammar, source)
        .ok_or_else(|| "the input does not parse".to_string())?;
    Ok(forms
        .forms
        .iter()
        .flat_map(|f| f.pattern.find_all(&tree))
        .map(|m| trimmed(source, m.node.start(), m.node.end()))
        .collect())
}

/// Where the engines may differ, by the place's prefix, and why. Each is
/// work left before the switch; a known difference that stops differing
/// fails the test, so the list stays true.
const KNOWN: &[(&str, &str)] = &[(
    "tests/corpus/rust/pattern_macro_contents/",
    "matches inside a macro's token tree (`pattern/macros.rs`) are not yet \
     found by colab's matcher",
)];

#[test]
fn both_engines_find_the_same_matches() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut differ = Vec::new();
    let mut compared = 0;
    for (place, lang, pattern, source) in cases(&root) {
        if source_language(&lang).is_none() {
            continue;
        }
        compared += 1;
        let (old, new) = (
            old_engine(&lang, &pattern, &source),
            new_engine(&lang, &pattern, &source),
        );
        let show = |r: &Ranges| -> Vec<String> {
            r.iter()
                .map(|&(s, e)| {
                    let t: String = source[s..e].chars().take(50).collect();
                    format!("{t:?}")
                })
                .collect()
        };
        let line = match (&old, &new) {
            (Ok(a), Ok(b)) if a == b => continue,
            (Ok(a), Ok(b)) => format!(
                "old only {:?}; new only {:?}",
                show(&a.difference(b).copied().collect()),
                show(&b.difference(a).copied().collect())
            ),
            (Err(_), Err(_)) => continue,
            (Ok(_), Err(e)) => format!("only the new engine refuses it: {e}"),
            (Err(e), Ok(_)) => format!("only the old engine refuses it: {e}"),
        };
        differ.push((place, format!("{lang}::pattern {pattern:?}: {line}")));
    }
    eprintln!("{compared} comparisons, {} differ", differ.len());
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
