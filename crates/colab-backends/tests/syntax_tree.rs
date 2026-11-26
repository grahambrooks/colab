//! `colab_syntax::Tree::from_tree_sitter` over every source file the repo
//! holds in a language colab parses with tree-sitter: the property samples
//! and the corpus cases (Rust is rust-analyzer's, `rust_parser.rs`). For
//! each file the colab tree must
//!
//! - be lossless: its tokens' texts, in order, are the file;
//! - be tree-sitter's tree: the same nodes, in the same order, with the
//!   same kinds, fields, named flags and ranges, once its trivia (the
//!   whitespace tree-sitter does not report) is set aside.
//!
//! This is the bar the tree must keep while the engine moves onto it.

mod common;

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use colab_syntax::{Grammar, Tree};

/// The language a file is parsed as, by extension: the languages still
/// read through tree-sitter, whose trees this conversion builds (Rust, Go,
/// Python, Java, JavaScript, TypeScript, C#, C++, C and PHP have parsers
/// of their own, held to their files by `rust_parser.rs`, `go_parser.rs`,
/// `python_parser.rs`, `java_parser.rs`, `js_parser.rs`,
/// `csharp_parser.rs`, `cpp_parser.rs` and `php_parser.rs`).
fn language_of(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        _ => return None,
    })
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, out);
        } else if language_of(&path).is_some() {
            out.push(path);
        }
    }
}

type Shape = Vec<(String, Option<String>, Range<usize>, bool)>;

/// tree-sitter's nodes in document order; the root's range is left out,
/// since the colab root spans the whole text.
fn shape_ts(tree: &tree_sitter::Tree) -> Shape {
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    let mut down = true;
    loop {
        if down {
            let n = cursor.node();
            let range = if cursor.depth() == 0 {
                0..0
            } else {
                n.byte_range()
            };
            out.push((
                n.kind().to_string(),
                cursor.field_name().map(str::to_string),
                range,
                n.is_named(),
            ));
            if cursor.goto_first_child() {
                continue;
            }
        }
        if cursor.goto_next_sibling() {
            down = true;
            continue;
        }
        if !cursor.goto_parent() {
            break;
        }
        down = false;
    }
    out
}

fn shape(tree: &Tree) -> Shape {
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

#[test]
fn the_colab_tree_is_tree_sitters_and_lossless_over_every_source_file() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["tests/samples", "tests/corpus", "crates"] {
        files_under(&root.join(dir), &mut files);
    }
    files.sort();
    let mut grammars: std::collections::HashMap<&str, (tree_sitter::Language, Arc<Grammar>)> =
        std::collections::HashMap::new();
    let mut checked = std::collections::BTreeMap::<&str, usize>::new();
    let mut failures = Vec::new();
    let (mut parsing, mut building) = (Duration::ZERO, Duration::ZERO);
    for path in &files {
        let Some(lang) = language_of(path) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let (language, grammar) = grammars.entry(lang).or_insert_with(|| {
            let language = common::grammar(lang);
            let grammar = Arc::new(Grammar::from_tree_sitter(lang, &language));
            (language, grammar)
        });
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(language).unwrap();
        let started = Instant::now();
        let ts = parser.parse(&text, None).unwrap();
        parsing += started.elapsed();
        let shown = path.strip_prefix(&root).unwrap_or(path).display();
        let started = Instant::now();
        let built = Tree::from_tree_sitter(&ts, &text, grammar.clone());
        building += started.elapsed();
        match built {
            Err(e) => failures.push(format!("{shown}: {e}")),
            Ok(tree) => {
                let rebuilt: String = tree.tokens().map(|t| t.text()).collect();
                if rebuilt != text {
                    failures.push(format!("{shown}: not lossless"));
                } else if shape(&tree) != shape_ts(&ts) {
                    let (ours, theirs) = (shape(&tree), shape_ts(&ts));
                    let at = ours.iter().zip(&theirs).position(|(a, b)| a != b);
                    failures.push(format!(
                        "{shown}: differs from tree-sitter at node {at:?}: {:?} vs {:?}",
                        at.map(|i| &ours[i]),
                        at.map(|i| &theirs[i]),
                    ));
                }
            }
        }
        *checked.entry(lang).or_default() += 1;
    }
    // The source languages still read through tree-sitter.
    assert!(
        checked.len() >= 2,
        "every tree-sitter source language has a file: {checked:?}"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // `--nocapture` shows what building the tree costs beside the parse.
    eprintln!(
        "{} files {checked:?}: tree-sitter parse {parsing:?}, colab tree {building:?}",
        files.len()
    );
}
