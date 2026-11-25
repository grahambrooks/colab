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
