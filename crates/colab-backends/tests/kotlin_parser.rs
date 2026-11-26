//! `colab_parse_kotlin`, colab's own Kotlin parser, held to
//! tree-sitter-kotlin-ng's trees while Kotlin moves off tree-sitter: over
//! every Kotlin file in the repo (or under `COLAB_KOTLIN_FILES`, a
//! directory, for a wider run), each tree must be lossless, and where
//! tree-sitter reads a file cleanly the two trees must hold the same nodes,
//! in the same order, with the same kinds, fields, named flags and ranges.
//! `COLAB_KOTLIN_SHOW=<n>` lists the first n differences (and clean files
//! read with an error).

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

fn kotlin_files(dir: &Path, out: &mut Vec<PathBuf>) {
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
            kotlin_files(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("kt" | "kts")
        ) {
            out.push(path);
        }
    }
}

fn tree_sitter_tree(text: &str) -> colab_syntax::Tree {
    colab_rewrite::syntax::tree(&tree_sitter_kotlin_ng::LANGUAGE.into(), text)
        .expect("tree-sitter parses")
}

#[test]
fn colab_kotlin_trees_are_lossless_and_tree_sitters() {
    let wide = std::env::var_os("COLAB_KOTLIN_FILES");
    let root = wide.clone().map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        PathBuf::from,
    );
    let mut files = Vec::new();
    kotlin_files(&root, &mut files);
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
        let ours = colab_parse_kotlin::parse(&text);
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
        let theirs = tree_sitter_tree(&text);
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
                        s.0.escape_debug(),
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
        "kotlin: {checked} files ({clean} clean to tree-sitter): {same} the same, {} differ, {} clean ones we read with an error; ours {ours_time:?}, tree-sitter {theirs_time:?}",
        differ.len(),
        error_files.len()
    );
    let show = std::env::var("COLAB_KOTLIN_SHOW")
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
        assert!(
            differ.is_empty(),
            "{} Kotlin files differ from tree-sitter",
            differ.len()
        );
    }
}
