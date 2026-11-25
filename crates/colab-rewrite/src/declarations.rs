//! A file's declarations from its tree-sitter tree, for
//! `colab edit --symbol` when no language server answers.
//!
//! One rule for every grammar: a node whose kind reads as a declaration
//! (`function_item`, `class_declaration`, `method_definition`,
//! `type_spec`, …) and that names itself (a `name` field) is one; nested
//! ones are its children. Two shapes get their names from elsewhere: a
//! Rust `impl` block is named by its type (`impl Display for Config` is
//! `Config`), and a method with a `receiver` (Go) sits under its receiver
//! type, so `Config/load` finds it as it finds a Rust method; a C or C++
//! function is named by its declarator (`int *f(void)` is `f`). A
//! declaration takes in the comments and attributes directly above it
//! (no blank line between), and the decorators or `export` that wrap it.

use colab_core::symbols::Declaration;
use colab_syntax::{Node, Tree};

/// Words in a node kind that make it a declaration, given a name.
const DECLARATION_WORDS: &[&str] = &[
    "function",
    "method",
    "class",
    "struct",
    "enum",
    "trait",
    "interface",
    "impl",
    "module",
    "mod_",
    "namespace",
    "object",
    "protocol",
    "type_spec",
    "type_alias",
    "type_item",
    "const_item",
    "static_item",
    "macro_definition",
    "constructor",
    "property_declaration",
    "union",
];

fn is_declaration_kind(kind: &str) -> bool {
    // A use is not a declaration: Java's `method_invocation` and
    // `method_reference` have a `name` field too.
    !kind.contains("call")
        && !kind.contains("invocation")
        && !kind.contains("reference")
        && !kind.contains("expression")
        && !kind.contains("type_identifier")
        && !kind.ends_with("_type")
        && DECLARATION_WORDS.iter().any(|w| kind.contains(w))
}

/// The declarations in `source`, parsed with `grammar`.
pub fn declarations(grammar: &tree_sitter::Language, source: &str) -> Vec<Declaration> {
    crate::syntax::tree(grammar, source).map_or_default(|tree| declarations_in(&tree))
}

/// The declarations in a colab tree (whichever parser made it), by its
/// kinds and fields.
pub fn declarations_in(tree: &Tree) -> Vec<Declaration> {
    let source = tree.text();
    let mut top = Vec::new();
    collect(tree.root(), source, &mut top);
    // Go methods are top-level nodes; put each under its receiver type.
    let mut out: Vec<Declaration> = Vec::new();
    let mut methods: Vec<(String, Declaration)> = Vec::new();
    for (receiver, d) in top {
        match receiver {
            Some(r) => methods.push((r, d)),
            None => out.push(d),
        }
    }
    for (receiver, method) in methods {
        match out.iter_mut().find(|d| d.name == receiver) {
            Some(owner) => owner.children.push(method),
            None => out.push(Declaration {
                name: receiver,
                kind: "receiver".into(),
                range: method.range.clone(),
                children: vec![method],
            }),
        }
    }
    out
}

/// Declarations at or below `node`, each with the receiver type it
/// belongs to, if it has one.
fn collect(node: Node<'_>, source: &str, out: &mut Vec<(Option<String>, Declaration)>) {
    for child in node.named_children() {
        match declaration(child, source) {
            Some((receiver, mut d)) => {
                let mut inner = Vec::new();
                collect(child, source, &mut inner);
                d.children = inner.into_iter().map(|(_, d)| d).collect();
                out.push((receiver, d));
            }
            None => collect(child, source, out),
        }
    }
}

fn text<'s>(node: Node<'_>, source: &'s str) -> &'s str {
    &source[node.range()]
}

/// The last identifier in `text`: the type in `*Config`, `Box<Config>`'s
/// `Config` aside, the name a reader would use.
fn last_word(text: &str) -> String {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .rfind(|w| !w.is_empty() && w.chars().next().is_some_and(|c| !c.is_ascii_digit()))
        .unwrap_or("")
        .to_string()
}

fn declaration(node: Node<'_>, source: &str) -> Option<(Option<String>, Declaration)> {
    let kind = node.kind_name();
    if !is_declaration_kind(kind) {
        return None;
    }
    let name = match node.child_by_field("name") {
        Some(n) => text(n, source).trim().to_string(),
        // A Rust `impl` is named by the type it is for.
        None if kind == "impl_item" => node
            .child_by_field("type")
            .map(|t| last_word(text(t, source)))?,
        // C and C++ name a function through its declarator chain:
        // `int *f(void)`, `void A::m()`.
        None if kind == "function_definition" => {
            let mut d = node.child_by_field("declarator")?;
            while let Some(inner) = d.child_by_field("declarator") {
                d = inner;
            }
            last_word(text(d, source))
        }
        None => return None,
    };
    if name.is_empty() {
        return None;
    }
    let receiver = node.child_by_field("receiver").map(|r| {
        // `(c *Config)`: the type, not the variable.
        let t = text(r, source);
        last_word(t)
    });
    Some((
        receiver,
        Declaration {
            name,
            kind: kind.to_string(),
            range: extent(node, source),
            children: Vec::new(),
        },
    ))
}

/// `node`'s bytes, widened to a wrapping decorator or export and to the
/// comments and attributes directly above it.
fn extent(node: Node<'_>, source: &str) -> std::ops::Range<usize> {
    let mut outer = node;
    while let Some(parent) = outer.parent() {
        let k = parent.kind_name();
        if k.contains("decorated") || k == "export_statement" {
            outer = parent;
        } else {
            break;
        }
    }
    let mut start = outer.start();
    // The sibling before, whitespace passed over.
    fn before(n: Node<'_>) -> Option<Node<'_>> {
        std::iter::successors(n.prev_sibling(), Node::prev_sibling).find(|s| !s.is_trivia())
    }
    let mut prev = before(outer);
    while let Some(p) = prev {
        let k = p.kind_name();
        // An inner doc comment (`//!`, `/*!`) or attribute (`#![…]`)
        // belongs to what encloses it, not to the next item.
        let inner = {
            let t = text(p, source);
            t.starts_with("//!") || t.starts_with("/*!") || t.starts_with("#!")
        };
        let leading =
            !inner && (k.contains("comment") || k.contains("attribute") || k == "decorator");
        // Directly above: nothing but one line break between.
        let gap = &source[p.end()..start];
        if !leading || gap.matches('\n').count() > 1 || !gap.trim().is_empty() {
            break;
        }
        start = p.start();
        prev = before(p);
    }
    start..outer.end()
}

#[cfg(test)]
mod tests {
    use super::*;
    use colab_core::symbols::{flatten, resolve};

    fn paths(grammar: tree_sitter::Language, source: &str) -> Vec<String> {
        let decls = declarations(&grammar, source);
        flatten(&decls).iter().map(|f| f.path.join("/")).collect()
    }

    // Rust's declarations come from rust-analyzer's typed AST
    // (`colab-lang-rust/src/own.rs`, which has the Rust cases).

    #[test]
    fn go_methods_sit_under_their_receiver_type() {
        let src = "package p\n\ntype Config struct{}\n\n// Load reads it.\nfunc (c *Config) Load() error { return nil }\n\nfunc main() {}\n";
        let grammar: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
        let decls = declarations(&grammar, src);
        let load = resolve(&decls, "Config/Load", src).unwrap();
        assert!(
            src[load.declaration.range.clone()].starts_with("// Load reads it.\nfunc (c *Config)")
        );
        assert!(resolve(&decls, "main", src).is_ok());
    }

    #[test]
    fn python_decorators_and_classes() {
        let src =
            "class A:\n    @property\n    def x(self):\n        return 1\n\ndef f():\n    pass\n";
        let grammar: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
        assert_eq!(paths(grammar.clone(), src), ["A", "A/x", "f"]);
        let decls = declarations(&grammar, src);
        let x = resolve(&decls, "A/x", src).unwrap();
        assert!(src[x.declaration.range.clone()].starts_with("@property\n    def x"));
    }
}
