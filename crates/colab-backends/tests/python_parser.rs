//! `colab_parse_python`, colab's own Python parser, compared with
//! tree-sitter-python's trees: over every `.py` file in the repo (or under
//! `COLAB_PY_FILES`, a directory, for a wider run), each tree must be
//! lossless; where tree-sitter reads a repo file cleanly, the first node at
//! which the trees differ is held to the reviewed baseline
//! `tests/parsers/python.diffs` (`COLAB_BLESS=1` rewrites it). tree-sitter
//! is a reference, not the target.
//! `COLAB_PY_SHOW=<n>` lists the first n differences.

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

fn py_files(dir: &Path, out: &mut Vec<PathBuf>) {
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
            py_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "py") {
            out.push(path);
        }
    }
}

#[test]
fn colab_python_trees_are_lossless_and_tree_sitters() {
    let wide = std::env::var_os("COLAB_PY_FILES");
    let root = wide.clone().map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        PathBuf::from,
    );
    let mut files = Vec::new();
    py_files(&root, &mut files);
    files.sort();
    let (mut ours_time, mut theirs_time) = (Duration::ZERO, Duration::ZERO);
    let (mut checked, mut clean, mut same) = (0usize, 0usize, 0usize);
    let mut lossy = Vec::new();
    let mut differ: Vec<String> = Vec::new();
    let mut ours_errors = 0usize;
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
        let ours = colab_parse_python::parse(&text);
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
        let theirs = common::tree_sitter_tree("python", &text);
        theirs_time += started.elapsed();
        if theirs.has_error() {
            continue;
        }
        clean += 1;
        if ours.has_error() {
            ours_errors += 1;
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
        "{checked} files ({clean} clean to tree-sitter): {same} the same, {} differ, {ours_errors} clean ones we read with an error; ours {ours_time:?}, tree-sitter {theirs_time:?}",
        differ.len()
    );
    let show = std::env::var("COLAB_PY_SHOW")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(25);
    for d in differ.iter().take(show) {
        eprintln!("{d}");
    }
    assert!(lossy.is_empty(), "not lossless:\n{}", lossy.join("\n"));
    if wide.is_none() {
        common::baseline("python", &differ);
    }
}
