//! `colab_syntax::pattern`, the matcher of colab's own, over every source
//! file the repo holds in the 14 languages, each read by the parser its
//! backend uses (tree-sitter, or rust-analyzer's for Rust). For a spread
//! of named nodes in each file:
//!
//! - **self-match**: the node, as an exact pattern, matches itself;
//! - **soundness**: everywhere the exact pattern matches, the code is the
//!   node's code, token for token, once the tokens the match reports as
//!   passed over are set aside;
//! - **names**: with every named token a distinct `$V<i>`, the pattern
//!   still matches the node and binds each to that token's text;
//! - **sequence**: with an inner node's children replaced by one `$$$S`,
//!   it matches and binds them all.
//!
//! These hold by construction if the matcher is right, so any failure is
//! a matcher bug, named with its file, node kind and text.

mod common;

use std::path::{Path, PathBuf};

use colab_syntax::Node;
use colab_syntax::pattern::{Binding, MetaVar, Pattern, PatternNode};

/// Nodes checked per file, at most, spread evenly over the file.
const PER_FILE: usize = 60;

fn language_of(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "cpp",
        "cs" => "csharp",
        "go" => "go",
        "java" => "java",
        "js" | "mjs" | "cjs" | "jsx" => "js",
        "kt" | "kts" => "kotlin",
        "php" => "php",
        "py" => "python",
        "rs" => "rust",
        "swift" => "swift",
        "ts" => "ts",
        "tsx" => "tsx",
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

fn visible(n: &Node<'_>) -> bool {
    !n.is_trivia() && !(n.is_extra() && !n.is_error())
}

/// The node's code tokens (no trivia, nothing inside a comment).
fn code(node: Node<'_>, skipped: &[Node<'_>]) -> Vec<String> {
    let mut out = Vec::new();
    let mut hidden_until = 0;
    for n in node.descendants() {
        if n.start() < hidden_until {
            continue;
        }
        if !visible(&n) || skipped.contains(&n) {
            hidden_until = n.end().max(n.start() + 1);
            continue;
        }
        if n.is_token() {
            out.push(n.text().to_string());
        }
    }
    out
}

/// `node` as a pattern, with each named token replaced by `$V<i>` when
/// `names` (counting in `next`).
fn exact(node: Node<'_>, names: bool, next: &mut usize) -> PatternNode {
    if node.is_token() {
        if names && node.is_named() {
            *next += 1;
            return PatternNode::Var(MetaVar::One {
                name: Some(format!("V{next}")),
                named_only: true,
            });
        }
        return PatternNode::Token {
            kind: node.kind(),
            text: node.text().to_string(),
            named: node.is_named(),
        };
    }
    PatternNode::Node {
        kind: node.kind(),
        children: node
            .children()
            .filter(visible)
            .map(|c| exact(c, names, next))
            .collect(),
    }
}

/// The named tokens of `node` in order, as `$V<i>` would bind them.
fn named_tokens(node: Node<'_>) -> Vec<String> {
    let mut out = Vec::new();
    let mut hidden_until = 0;
    for n in node.descendants() {
        if n.start() < hidden_until {
            continue;
        }
        if !visible(&n) {
            hidden_until = n.end().max(n.start() + 1);
            continue;
        }
        if n.is_token() && n.is_named() {
            out.push(n.text().to_string());
        }
    }
    out
}

#[test]
fn the_matcher_holds_its_properties_over_every_source_file() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["tests/samples", "tests/corpus", "crates"] {
        files_under(&root.join(dir), &mut files);
    }
    files.sort();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for path in &files {
        let lang = language_of(path).unwrap();
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let tree = common::tree(lang, &text);
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let candidates: Vec<Node<'_>> = tree
            .nodes()
            .filter(|n| n.is_named() && visible(n) && n.id() != 0)
            .collect();
        let step = (candidates.len() / PER_FILE).max(1);
        for node in candidates.iter().step_by(step).take(PER_FILE) {
            checked += 1;
            let mut fail = |what: &str| {
                let snippet: String = node.text().chars().take(60).collect();
                failures.push(format!(
                    "{shown}: {what} `{}` {snippet:?}",
                    node.kind_name()
                ));
            };
            // Self-match and soundness.
            let pattern = Pattern::new(exact(*node, false, &mut 0));
            if pattern.matches(*node).is_none() {
                fail("self-match");
                continue;
            }
            let want = code(*node, &[]);
            for m in pattern.find_all(&tree) {
                if code(m.node, &m.skipped) != want {
                    fail("soundness");
                    break;
                }
            }
            // Names.
            let mut count = 0;
            let generalized = Pattern::new(exact(*node, true, &mut count));
            match generalized.matches(*node) {
                None => fail("names: no match"),
                Some(m) => {
                    let bound: Vec<String> = m
                        .bindings
                        .iter()
                        .map(|(_, b)| b.text().to_string())
                        .collect();
                    if bound != named_tokens(*node) {
                        fail("names: bindings");
                    }
                }
            }
            // Sequence.
            if !node.is_token() {
                let seq = Pattern::new(PatternNode::Node {
                    kind: node.kind(),
                    children: vec![PatternNode::Var(MetaVar::Many {
                        name: Some("S".into()),
                    })],
                });
                match seq.matches(*node) {
                    None => fail("sequence: no match"),
                    Some(m) => {
                        // Every child but trivia, comments included (a
                        // sequence takes the comments beside it), and
                        // no whitespace at the end.
                        let kids: Vec<Node<'_>> =
                            node.children().filter(|c| !c.is_trivia()).collect();
                        let expected = match (kids.first(), kids.last()) {
                            (Some(a), Some(b)) => text[a.start()..b.end()].trim_end(),
                            _ => "",
                        };
                        if m.get("S").map(Binding::text) != Some(expected) {
                            fail("sequence: binding");
                        }
                    }
                }
            }
        }
    }
    eprintln!("{checked} nodes over {} files", files.len());
    assert!(checked > 5000, "too few nodes checked: {checked}");
    assert!(
        failures.is_empty(),
        "{} of {checked} failed:\n{}",
        failures.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
