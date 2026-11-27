//! `colab_parse_c`, colab's own C++ and C parser, compared with
//! tree-sitter-cpp's and tree-sitter-c's trees: over every C++ (C) file in
//! the repo (or under `COLAB_CPP_FILES` / `COLAB_C_FILES`, a directory, for
//! a wider run), each tree must be lossless; where tree-sitter reads a repo
//! file cleanly, the first node at which the trees differ is held to the
//! reviewed baseline `tests/parsers/{cpp,c}.diffs` (`COLAB_BLESS=1`
//! rewrites it). tree-sitter is a reference, not the target.
//! `COLAB_CPP_SHOW=<n>` lists the first n differences (and clean files read
//! with an error).

mod common;

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

type Shape = (String, Option<String>, Range<usize>, bool);

fn shape(tree: &colab_syntax::Tree) -> Vec<Shape> {
    tree.nodes()
        .filter(|n| !n.is_trivia())
        .map(|n| {
            let range = if n.id() == 0 { 0..0 } else { n.range() };
            (
                n.kind_name().to_string(),
                n.field_name().map(str::to_string),
                range,
                n.is_named(),
            )
        })
        .collect()
}

fn cpp_files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            cpp_files(&path, exts, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
        {
            out.push(path);
        }
    }
}

#[test]
fn colab_cpp_trees_are_lossless_and_tree_sitters() {
    compare(
        colab_parse_c::Dialect::Cpp,
        "cpp",
        &["cpp", "cc", "cxx", "hpp", "hh", "h"],
        "COLAB_CPP_FILES",
    );
}

#[test]
fn colab_c_trees_are_lossless_and_tree_sitters() {
    compare(colab_parse_c::Dialect::C, "c", &["c", "h"], "COLAB_C_FILES");
}

/// Compare `dialect`'s trees with tree-sitter's grammar `lang` over the
/// files with `exts`, under `$env` or the repo.
fn compare(dialect: colab_parse_c::Dialect, lang: &str, exts: &[&str], env: &str) {
    let wide = std::env::var_os(env);
    let root = wide.clone().map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        PathBuf::from,
    );
    let mut files = Vec::new();
    cpp_files(&root, exts, &mut files);
    files.sort();
    let (mut ours_time, mut theirs_time) = (Duration::ZERO, Duration::ZERO);
    let (mut checked, mut clean, mut same) = (0usize, 0usize, 0usize);
    let mut lossy = Vec::new();
    let mut differ: Vec<String> = Vec::new();
    let mut error_files: Vec<String> = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        checked += 1;
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let started = Instant::now();
        let ours = colab_parse_c::parse_as(&text, dialect);
        ours_time += started.elapsed();
        let Ok(ours) = ours else {
            lossy.push(format!("{shown}: no tree"));
            continue;
        };
        let rebuilt: String = ours.tokens().map(|t| t.text()).collect();
        if rebuilt != text {
            lossy.push(shown.clone());
            continue;
        }
        let started = Instant::now();
        let theirs = common::tree_sitter_tree(lang, &text);
        theirs_time += started.elapsed();
        if theirs.has_error() {
            continue;
        }
        clean += 1;
        if ours.has_error() {
            let at = ours
                .nodes()
                .find(|n| n.is_error() || n.is_missing())
                .map_or(0, |n| n.start());
            let line = text[..at].matches('\n').count() + 1;
            error_files.push(format!("{shown}: error at l{line}"));
        }
        let (a, b) = (shape(&ours), shape(&theirs));
        if a == b {
            same += 1;
            continue;
        }
        let at = a
            .iter()
            .zip(&b)
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        let line = |r: &Range<usize>| text[..r.start.min(text.len())].matches('\n').count() + 1;
        let context = |s: Option<&Shape>| {
            s.map_or_else(
                || "(none)".to_string(),
                |s| {
                    format!(
                        "{} {:?} {:?} l{} {:?}",
                        s.0,
                        s.1,
                        s.2,
                        line(&s.2),
                        text[s.2.clone()].chars().take(40).collect::<String>()
                    )
                },
            )
        };
        differ.push(format!(
            "{shown}: node {at}\n    ours   {}\n    theirs {}",
            context(a.get(at)),
            context(b.get(at))
        ));
    }
    eprintln!(
        "{lang}: {checked} files ({clean} clean to tree-sitter): {same} the same, {} differ, {} clean ones we read with an error; ours {ours_time:?}, tree-sitter {theirs_time:?}",
        differ.len(),
        error_files.len()
    );
    let show = std::env::var("COLAB_CPP_SHOW")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(25);
    for d in differ.iter().take(show) {
        eprintln!("{d}");
    }
    for e in error_files.iter().take(show) {
        eprintln!("{e}");
    }
    assert!(lossy.is_empty(), "not lossless:\n{}", lossy.join("\n"));
    if wide.is_none() {
        common::baseline(lang, &differ);
    }
}
