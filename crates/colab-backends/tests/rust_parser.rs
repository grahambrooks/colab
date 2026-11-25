//! `colab_parse_rust` (rust-analyzer's parser) over real Rust: every `.rs`
//! file in the repo, or under `COLAB_RUST_FILES` (a directory) for a wider
//! one-off run. Each tree must be lossless, and the parser must call a file
//! clean exactly when tree-sitter does, or the difference is printed:
//! that is the bar for parsing Rust without tree-sitter.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every `rust::pattern` text the repo's corpus, conformance cases,
/// examples and evals spell, in either quote.
fn rust_patterns(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for dir in ["tests/corpus", "tests/conformance", "examples", "evals"] {
        walk(&root.join(dir), &mut files);
    }
    let mut patterns = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (at, _) in text.match_indices("rust::pattern ") {
            let rest = &text[at + "rust::pattern ".len()..];
            let Some(quote) = rest.chars().next().filter(|c| *c == '\'' || *c == '"') else {
                continue;
            };
            if let Some(end) = rest[1..].find(quote) {
                patterns.push(rest[1..=end].to_string());
            }
        }
    }
    patterns.sort();
    patterns.dedup();
    patterns
}

/// How far moving Rust's pattern engine onto rust-analyzer's trees would
/// change what users get: each of the repo's Rust patterns is compiled
/// and run over the repo's Rust with each parser, and the match spans are
/// compared. A measurement, printed, not yet a bar: kind names, macro
/// contents and the backend's tree-sitter-named rules all differ.
#[test]
fn rust_patterns_on_rust_analyzer_trees() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ts = (colab_lang_rust::LANG.pattern)();
    let ra = colab_lang_rust::on_rust_analyzer();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    let sources: Vec<String> = files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();
    let (mut same, mut differ, mut refused) = (0, 0, 0);
    // A lone metavariable matches every node, so it measures only how
    // finely each parser divides the text.
    for text in rust_patterns(&root).into_iter().filter(|t| {
        !t.trim().starts_with('$')
            || t.trim()
                .contains(|c: char| !c.is_alphanumeric() && c != '$' && c != '_')
    }) {
        let compiled = (
            colab_rewrite::syntax::compile_forms(&ts, &text),
            colab_rewrite::syntax::compile_forms(&ra, &text),
        );
        let (Ok(ts_forms), Ok(ra_forms)) = compiled else {
            refused += 1;
            eprintln!(
                "refused {text:?}: tree-sitter {}, rust-analyzer {}",
                compiled.0.as_ref().map_or_else(String::as_str, |_| "ok"),
                compiled.1.as_ref().map_or_else(String::as_str, |_| "ok"),
            );
            continue;
        };
        let (mut ts_hits, mut ra_hits, mut common) = (0usize, 0usize, 0usize);
        for source in &sources {
            // A match by where it ends and what it binds: rust-analyzer's
            // items hold their attributes and docs, so they start earlier.
            let spans = |language, forms| -> Vec<(usize, Vec<(String, String)>)> {
                colab_rewrite::syntax::find(language, forms, source)
                    .into_iter()
                    .map(|f| (f.range.start + f.text.trim_end().len(), f.captures))
                    .collect()
            };
            let a = spans(&ts, &ts_forms);
            let b = spans(&ra, &ra_forms);
            ts_hits += a.len();
            ra_hits += b.len();
            common += a.iter().filter(|r| b.contains(r)).count();
        }
        if ts_hits == common && ra_hits == common {
            same += 1;
        } else {
            differ += 1;
            eprintln!("{text:?}: tree-sitter {ts_hits}, rust-analyzer {ra_hits}, both {common}");
        }
    }
    eprintln!("{same} patterns match the same spans, {differ} differ, {refused} refused");
}

#[test]
fn rust_analyzer_trees_are_lossless_and_agree_on_errors() {
    let root = std::env::var_os("COLAB_RUST_FILES").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        PathBuf::from,
    );
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    files.sort();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&common::grammar("rust")).unwrap();
    let (mut ra_time, mut ts_time) = (Duration::ZERO, Duration::ZERO);
    let (mut checked, mut bytes) = (0usize, 0usize);
    let mut lossy = Vec::new();
    let (mut only_ra_errors, mut only_ts_errors) = (Vec::new(), Vec::new());
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        checked += 1;
        bytes += text.len();
        let started = Instant::now();
        let tree = colab_parse_rust::parse(&text);
        ra_time += started.elapsed();
        let started = Instant::now();
        let ts = parser.parse(&text, None).unwrap();
        ts_time += started.elapsed();
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let Ok(tree) = tree else {
            lossy.push(format!("{shown}: no tree"));
            continue;
        };
        let rebuilt: String = tree.tokens().map(|t| t.text()).collect();
        if rebuilt != text {
            lossy.push(shown.clone());
        }
        match (tree.has_error(), ts.root_node().has_error()) {
            (true, false) => only_ra_errors.push(shown),
            (false, true) => only_ts_errors.push(shown),
            _ => {}
        }
    }
    eprintln!(
        "{checked} files, {:.1} MB: rust-analyzer {ra_time:?}, tree-sitter {ts_time:?}",
        bytes as f64 / 1e6
    );
    eprintln!(
        "errors only rust-analyzer sees: {} {:?}",
        only_ra_errors.len(),
        only_ra_errors.iter().take(8).collect::<Vec<_>>()
    );
    eprintln!(
        "errors only tree-sitter sees: {} {:?}",
        only_ts_errors.len(),
        only_ts_errors.iter().take(8).collect::<Vec<_>>()
    );
    assert!(checked > 100, "too few files: {checked}");
    assert!(lossy.is_empty(), "not lossless:\n{}", lossy.join("\n"));
}
