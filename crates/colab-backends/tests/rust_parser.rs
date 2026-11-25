//! `colab_parse_rust` (rust-analyzer's parser) over real Rust: every `.rs`
//! file in the repo, or under `COLAB_RUST_FILES` (a directory) for a wider
//! one-off run. Each tree must be lossless, and every file of the repo's
//! own must parse cleanly.
//!
//! While Rust moved off tree-sitter this file also held rust-analyzer's
//! trees to tree-sitter's: which files have errors (over the Cargo
//! registry's 122,558 files, 5,347 that tree-sitter read with an error
//! parsed cleanly here, 51 the other way), how the repo's 27 Rust patterns
//! match on each (the same, once `method_calls`, the aliases and the
//! fields were in), and the alignment of kinds and fields that
//! `colab_parse_rust`'s `names` comes from (docs/development-plan.md,
//! step 3; the comparisons are in the history before tree-sitter-rust
//! was dropped).

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

#[test]
fn rust_analyzer_trees_are_lossless_and_the_repos_rust_parses() {
    let wide = std::env::var_os("COLAB_RUST_FILES");
    let root = wide.clone().map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        PathBuf::from,
    );
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    files.sort();
    let (mut elapsed, mut checked, mut bytes) = (Duration::ZERO, 0usize, 0usize);
    let (mut lossy, mut with_errors) = (Vec::new(), Vec::new());
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        checked += 1;
        bytes += text.len();
        let started = Instant::now();
        let tree = colab_parse_rust::parse(&text);
        elapsed += started.elapsed();
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
        if tree.has_error() {
            with_errors.push(shown);
        }
    }
    eprintln!(
        "{checked} files, {:.1} MB in {elapsed:?}; with syntax errors: {} {:?}",
        bytes as f64 / 1e6,
        with_errors.len(),
        with_errors.iter().take(8).collect::<Vec<_>>()
    );
    assert!(checked > 100, "too few files: {checked}");
    assert!(lossy.is_empty(), "not lossless:\n{}", lossy.join("\n"));
    if wide.is_none() {
        assert!(
            with_errors.is_empty(),
            "the repo's Rust parses cleanly:\n{}",
            with_errors.join("\n")
        );
    }
}
