use std::sync::Arc;

use super::*;

fn grammar() -> Arc<Grammar> {
    Arc::new(Grammar::new(
        "toy",
        vec![
            ("file".into(), true),
            ("call".into(), true),
            ("ident".into(), true),
            ("(".into(), false),
            (")".into(), false),
        ],
        vec!["".into(), "function".into()],
    ))
}

const NAMED: Flags = Flags {
    token: false,
    named: true,
    missing: false,
    extra: false,
};
const ANON: Flags = Flags {
    named: false,
    ..NAMED
};

/// `  f(x)  ` as file(call(ident `f` [function], `(`, ident `x`, `)`)).
fn toy() -> Tree {
    let mut b = Builder::new("  f(x)  ", grammar());
    b.start_node(Kind(0), None, NAMED).unwrap();
    b.trivia_to(2).unwrap();
    b.start_node(Kind(1), None, NAMED).unwrap();
    b.token(Kind(2), Some(Field(1)), NAMED, 1).unwrap();
    b.token(Kind(3), None, ANON, 1).unwrap();
    b.token(Kind(2), None, NAMED, 1).unwrap();
    b.token(Kind(4), None, ANON, 1).unwrap();
    b.finish_node().unwrap();
    b.finish().unwrap()
}

#[test]
fn the_tokens_are_the_text() {
    let tree = toy();
    let text: String = tree.tokens().map(|t| t.text()).collect();
    assert_eq!(text, "  f(x)  ");
    assert_eq!(tree.root().range(), 0..8);
}

#[test]
fn a_node_spans_its_children_and_trivia_stays_outside() {
    let tree = toy();
    let call = tree.root().named_children().next().unwrap();
    assert_eq!(call.kind_name(), "call");
    assert_eq!(call.range(), 2..6);
    assert_eq!(call.text(), "f(x)");
    // Leading and trailing whitespace are the root's trivia.
    let kinds: Vec<&str> = tree.root().children().map(|c| c.kind_name()).collect();
    assert_eq!(kinds, ["trivia", "call", "trivia"]);
}

#[test]
fn fields_siblings_parents_and_descendants() {
    let tree = toy();
    let call = tree.root().named_children().next().unwrap();
    let f = call.child_by_field("function").unwrap();
    assert_eq!(f.text(), "f");
    assert_eq!(f.field_name(), Some("function"));
    assert_eq!(f.parent(), Some(call));
    assert_eq!(f.next_sibling().unwrap().text(), "(");
    assert_eq!(
        call.last_child().unwrap().prev_sibling().unwrap().text(),
        "x"
    );
    let names: Vec<&str> = call.named_children().map(|c| c.text()).collect();
    assert_eq!(names, ["f", "x"]);
    assert_eq!(call.descendants().count(), 5);
    assert_eq!(tree.root().descendants().count(), tree.len());
    assert!(call.child_by_field("arguments").is_none());
}

#[test]
fn covering_finds_the_smallest_node() {
    let tree = toy();
    let root = tree.root();
    assert_eq!(root.covering(4..5).unwrap().text(), "x");
    assert_eq!(root.covering(2..6).unwrap().kind_name(), "call");
    assert_eq!(root.covering(3..5).unwrap().kind_name(), "call");
    assert!(root.covering(0..20).is_none());
}

#[test]
fn misuse_is_an_error_not_a_panic() {
    let mut b = Builder::new("ab", grammar());
    assert!(b.token(Kind(2), None, NAMED, 1).is_err(), "no root");
    b.start_node(Kind(0), None, NAMED).unwrap();
    assert!(b.token(Kind(2), None, NAMED, 3).is_err(), "past the end");
    assert!(b.finish_node().is_err(), "the root closes in finish");
    let mut b = Builder::new("é", grammar());
    b.start_node(Kind(0), None, NAMED).unwrap();
    assert!(
        b.token(Kind(2), None, NAMED, 1).is_err(),
        "inside a character"
    );
    let mut b = Builder::new("x", grammar());
    b.start_node(Kind(0), None, NAMED).unwrap();
    b.start_node(Kind(1), None, NAMED).unwrap();
    assert!(b.finish().is_err(), "a node left open");
}

#[test]
fn a_deep_tree_builds_walks_and_drops_without_recursion() {
    let depth = 200_000;
    // One token per level below the root, so the last node is the deepest.
    let text = "x".repeat(depth - 1);
    let mut b = Builder::new(text, grammar());
    b.start_node(Kind(0), None, NAMED).unwrap();
    for _ in 1..depth {
        b.start_node(Kind(1), None, NAMED).unwrap();
        b.token(Kind(2), None, NAMED, 1).unwrap();
    }
    for _ in 1..depth {
        b.finish_node().unwrap();
    }
    let tree = b.finish().unwrap();
    assert_eq!(tree.root().descendants().count(), tree.len());
    let mut deepest = tree.nodes().last().unwrap();
    let mut levels = 0;
    while let Some(p) = deepest.parent() {
        deepest = p;
        levels += 1;
    }
    // The token, inside `depth - 1` calls, inside the root.
    assert_eq!(levels, depth);
}

#[cfg(feature = "tree-sitter")]
mod from_tree_sitter {
    use super::*;

    fn both(language: tree_sitter::Language, name: &str, text: &str) -> (tree_sitter::Tree, Tree) {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language).unwrap();
        let ts = parser.parse(text, None).unwrap();
        let grammar = Arc::new(Grammar::from_tree_sitter(name, &language));
        let tree = Tree::from_tree_sitter(&ts, text, grammar).unwrap();
        (ts, tree)
    }

    /// Every tree-sitter node, in document order, as (kind, field, range,
    /// named); and the same for the colab tree without its trivia.
    fn shape_ts(tree: &tree_sitter::Tree) -> Vec<(String, Option<String>, Range<usize>, bool)> {
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
            }
            if down && cursor.goto_first_child() {
                continue;
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

    fn shape(tree: &Tree) -> Vec<(String, Option<String>, Range<usize>, bool)> {
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

    const RUST: &str = "// leading\nuse std::fmt;\n\n/// Doc.\npub struct S { a: u32 }\n\nfn broken(a: i32) -> i32 {\n    a + ( let = ;\n}\n\n/* block */ fn last() -> usize { f(1, \"é\") }\n";

    #[test]
    fn a_rust_parse_keeps_every_node_and_every_byte() {
        let (ts, tree) = both(tree_sitter_rust::LANGUAGE.into(), "rust", RUST);
        assert_eq!(shape(&tree), shape_ts(&ts));
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, RUST);
        assert!(tree.has_error());
        let error = tree.nodes().find(Node::is_error).unwrap();
        assert!(error.text().contains("let"), "{error:?}");
        // The function after the error is whole.
        let last = tree
            .nodes()
            .filter(|n| n.kind_name() == "function_item")
            .last()
            .unwrap();
        assert_eq!(last.child_by_field("name").unwrap().text(), "last");
    }

    #[test]
    fn a_go_parse_keeps_its_fields() {
        let src = "package p\n\nfunc f() {\n\tlog.Println(\"a\", 1)\n}\n";
        let (ts, tree) = both(tree_sitter_go::LANGUAGE.into(), "go", src);
        assert_eq!(shape(&tree), shape_ts(&ts));
        let call = tree
            .nodes()
            .find(|n| n.kind_name() == "call_expression")
            .unwrap();
        assert_eq!(
            call.child_by_field("function").unwrap().text(),
            "log.Println"
        );
        assert_eq!(
            call.child_by_field("arguments").unwrap().text(),
            "(\"a\", 1)"
        );
    }
}
