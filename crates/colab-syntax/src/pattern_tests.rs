//! The matcher on real parses (Rust and Go through tree-sitter).

use std::sync::Arc;

use crate::pattern::{MetaVar, Pattern, expand, meta_variable};
use crate::{Grammar, Node, Tree};

fn parse(language: tree_sitter::Language, name: &str, text: &str) -> Tree {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language).unwrap();
    let ts = parser.parse(text, None).unwrap();
    let grammar = Arc::new(Grammar::from_tree_sitter(name, &language));
    Tree::from_tree_sitter(&ts, text, grammar).unwrap()
}

/// The deepest node spanning exactly `range`, through same-span wrappers
/// (`expression_statement` → `call_expression`).
fn spanning(tree: &Tree, range: std::ops::Range<usize>) -> Node<'_> {
    let mut node = tree.root().covering(range.clone()).unwrap();
    assert_eq!(node.range(), range, "the pattern is one node");
    while let Some(inner) = node
        .children()
        .find(|c| c.range() == range && c.is_named() && !c.is_token())
    {
        node = inner;
    }
    node
}

/// `pattern` compiled in a context: `wrap` holds `@` where it goes.
fn compile(language: tree_sitter::Language, name: &str, wrap: &str, pattern: &str) -> Pattern {
    let expanded = expand(pattern, 'µ');
    let at = wrap.find('@').unwrap();
    let text = wrap.replacen('@', &expanded, 1);
    let tree = parse(language, name, &text);
    Pattern::from_node(spanning(&tree, at..at + expanded.len()), 'µ')
}

fn rust(pattern: &str) -> Pattern {
    compile(
        tree_sitter_rust::LANGUAGE.into(),
        "rust",
        "fn _p() { @; }",
        pattern,
    )
}

fn go(pattern: &str) -> Pattern {
    compile(
        tree_sitter_go::LANGUAGE.into(),
        "go",
        "package p\nfunc _p() {\n@\n}\n",
        pattern,
    )
}

fn rust_tree(src: &str) -> Tree {
    parse(tree_sitter_rust::LANGUAGE.into(), "rust", src)
}

fn go_tree(src: &str) -> Tree {
    parse(tree_sitter_go::LANGUAGE.into(), "go", src)
}

/// Each match's text and its bindings as `NAME=text`.
fn found(pattern: &Pattern, tree: &Tree) -> Vec<(String, Vec<String>)> {
    pattern
        .find_all(tree)
        .iter()
        .map(|m| {
            let bound = m
                .bindings
                .iter()
                .map(|(n, b)| format!("{n}={}", b.text()))
                .collect();
            (m.node.text().to_string(), bound)
        })
        .collect()
}

#[test]
fn metavariables_are_read_as_colab_writes_them() {
    let one = |n: &str, named_only| MetaVar::One {
        name: Some(n.into()),
        named_only,
    };
    assert_eq!(meta_variable("µX", 'µ'), Some(one("X", true)));
    assert_eq!(meta_variable("µµX1", 'µ'), Some(one("X1", false)));
    assert_eq!(
        meta_variable("µ_", 'µ'),
        Some(MetaVar::One {
            name: None,
            named_only: true
        })
    );
    assert_eq!(
        meta_variable("µµµARGS", 'µ'),
        Some(MetaVar::Many {
            name: Some("ARGS".into())
        })
    );
    assert_eq!(
        meta_variable("µµµ", 'µ'),
        Some(MetaVar::Many { name: None })
    );
    assert_eq!(
        meta_variable("µµµB;", 'µ'),
        Some(MetaVar::Many {
            name: Some("B".into())
        })
    );
    assert_eq!(meta_variable("µx", 'µ'), None, "lower case is code");
    assert_eq!(meta_variable("$X", '$'), Some(one("X", true)));
    assert_eq!(expand("f($A, $$$B, \"$5\")", 'µ'), "f(µA, µµµB, \"$5\")");
    assert_eq!(expand("f($A)", '$'), "f($A)");
}

#[test]
fn a_call_binds_its_arguments() {
    let tree = rust_tree("fn m() { foo(1, x + 2); bar(3); foo(4); }");
    assert_eq!(
        found(&rust("foo($A, $B)"), &tree),
        [("foo(1, x + 2)".into(), vec!["A=1".into(), "B=x + 2".into()])]
    );
}

#[test]
fn every_named_child_is_accounted_for() {
    let tree = go_tree("package p\nfunc f() {\n\tif a { x() }\n\tif b { y() } else { z() }\n}\n");
    let hits = found(&go("if $C { $$$B }"), &tree);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].1, ["C=a", "B=x()"]);
    // Spelled, the `else` matches.
    assert_eq!(found(&go("if $C { $$$B } else { $$$E }"), &tree).len(), 1);
}

#[test]
fn a_syntax_error_is_taken_only_by_a_capture() {
    let tree = go_tree("package p\nfunc f() {\n\told(x :: y)\n\told(z)\n}\n");
    assert!(tree.has_error());
    let one: Vec<_> = found(&go("old($A)"), &tree)
        .into_iter()
        .map(|h| h.0)
        .collect();
    assert_eq!(one, ["old(z)"]);
    let all = found(&go("old($$$A)"), &tree);
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[0].1, ["A=x :: y"]);
}

#[test]
fn a_trailing_comma_is_passed_over_and_reported() {
    let tree = rust_tree("fn m() { foo(1, 2,); }");
    let pattern = rust("foo($A, $B)");
    let matches = pattern.find_all(&tree);
    assert_eq!(matches.len(), 1);
    let skipped: Vec<&str> = matches[0].skipped.iter().map(Node::text).collect();
    assert_eq!(skipped, [","]);
    // A token the pattern spells must be there.
    assert!(
        rust("foo($A, $B,)")
            .find_all(&rust_tree("fn m() { foo(1, 2); }"))
            .is_empty()
    );
}

#[test]
fn a_sequence_backtracks_to_fit_what_follows() {
    let tree = rust_tree("fn m() { f(1, 2, 3); f(9); }");
    assert_eq!(
        found(&rust("f($$$A, $L)"), &tree),
        [("f(1, 2, 3)".into(), vec!["A=1, 2".into(), "L=3".into()])]
    );
    let all = found(&rust("f($$$A)"), &tree);
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].1, ["A=9"]);
    // An empty sequence matches nothing.
    let empty = found(&rust("g($$$A)"), &rust_tree("fn m() { g(); }"));
    assert_eq!(empty, [("g()".into(), vec!["A=".into()])]);
}

#[test]
fn a_repeated_metavariable_is_the_same_code() {
    let tree = rust_tree("fn m() { a == a; a == b; f(x,y) == f(x, /* c */ y); }");
    let hits: Vec<_> = found(&rust("$X == $X"), &tree)
        .into_iter()
        .map(|h| h.0)
        .collect();
    assert_eq!(hits, ["a == a", "f(x,y) == f(x, /* c */ y)"]);
}

#[test]
fn comments_are_invisible_to_matching() {
    let tree = rust_tree("fn m() { foo(/* why */ 1); }");
    assert_eq!(
        found(&rust("foo($A)"), &tree),
        [("foo(/* why */ 1)".into(), vec!["A=1".into()])]
    );
}

#[test]
fn matches_come_outer_first_and_include_nested_ones() {
    let tree = rust_tree("fn m() { f(f(1)); }");
    let hits: Vec<_> = found(&rust("f($A)"), &tree)
        .into_iter()
        .map(|h| h.0)
        .collect();
    assert_eq!(hits, ["f(f(1))", "f(1)"]);
}

#[test]
fn a_lone_metavariable_takes_any_named_node() {
    let tree = rust_tree("fn m() { 1; }");
    let pattern = rust("$X");
    assert!(pattern.kind().is_none());
    let all = pattern.find_all(&tree);
    assert!(all.iter().all(|m| m.node.is_named()));
    assert!(all.iter().any(|m| m.node.text() == "1"));
}

#[test]
fn a_named_token_keeps_its_kind() {
    // `x` the identifier is not `x` the field name.
    let tree = rust_tree("fn m() { x; s.x; }");
    let hits: Vec<_> = found(&rust("x"), &tree).into_iter().map(|h| h.0).collect();
    assert_eq!(hits, ["x"], "the identifier, not the field");
    let field = tree
        .nodes()
        .find(|n| n.kind_name() == "field_identifier")
        .unwrap();
    assert!(rust("x").matches(field).is_none());
}
