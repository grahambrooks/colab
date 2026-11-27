//! Java source as a [`colab_syntax::Tree`], read by a parser of colab's
//! own: pure Rust, lossless, error-tolerant. Step 3 of the move off
//! tree-sitter (docs/development-plan.md).
//!
//! `lexer.rs` reads Java's tokens; `parser.rs` reads tree-sitter-java's
//! grammar by recursive descent into a tree whose kinds and fields are
//! named as `names.rs` says (tree-sitter-java's, for now). Comments are
//! put in the deepest node with code on both sides of them; whitespace is
//! trivia.

mod lexer;
mod names;
mod parser;

use std::sync::{Arc, OnceLock};

use colab_syntax::emit::{self, Names};
use colab_syntax::{BuildError, Grammar, SourceParser, Tree};

use parser::{Item, Tok};

/// colab's Java parser.
pub struct JavaParser;

/// The one parser; it holds nothing but its grammar.
pub static PARSER: JavaParser = JavaParser;

fn names() -> &'static Names {
    static NAMES: OnceLock<Names> = OnceLock::new();
    NAMES.get_or_init(|| Names::new("java", names::NAMED, names::ANONYMOUS, names::FIELDS))
}

colab_syntax::element!(Item, Tok, |n| named: true, extra: false);

/// `text` as a colab tree.
pub fn parse(text: &str) -> Result<Tree, BuildError> {
    let lexed = lexer::lex(text);
    let mut p = parser::Parser::new(text, lexed.tokens);
    let mut root = match p.program() {
        Item::Node(n) => n,
        Item::Tok(_) => unreachable!("a file is a node"),
    };
    let mut extras: Vec<(usize, usize, &'static str)> = lexed
        .extras
        .iter()
        .map(|e| (e.start, e.end, e.kind))
        .collect();
    extras.sort_unstable();
    emit::place_extras(&mut root.items, emit::comments(extras));
    emit::build(
        text,
        names(),
        "program",
        &root.items,
        p.errors || lexed.errors,
    )
}

impl SourceParser for JavaParser {
    fn grammar(&self) -> Arc<Grammar> {
        names().grammar()
    }

    fn parse(&self, text: &str) -> Tree {
        parse(text).unwrap_or_else(|_| emit::fallback(text, names(), "program"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "package p;\n\n// lead\npublic class A {\n    /** doc */\n    int f(int x) {\n        return x >> 1; // trail\n    }\n}\n";

    #[test]
    fn a_parse_is_lossless_and_clean() {
        let tree = parse(SRC).unwrap();
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, SRC);
        assert!(!tree.has_error());
    }

    #[test]
    fn comments_go_where_code_is_on_both_sides() {
        let tree = parse(SRC).unwrap();
        let parent = |text: &str| {
            tree.nodes()
                .find(|n| n.text() == text)
                .and_then(|n| n.parent())
                .map(|p| p.kind_name())
        };
        assert_eq!(parent("// lead"), Some("program"));
        assert_eq!(parent("/** doc */"), Some("class_body"));
        assert_eq!(parent("// trail"), Some("block"));
    }

    #[test]
    fn a_syntax_error_is_kept_and_the_rest_is_read() {
        let src = "class A {\n    void f() { int x = ; }\n    void g() {}\n}\n";
        let tree = parse(src).unwrap();
        assert!(tree.has_error());
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, src);
        assert!(
            tree.nodes()
                .any(|n| n.kind_name() == "method_declaration" && n.text() == "void g() {}")
        );
    }
}
