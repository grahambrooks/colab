//! Python source as a [`colab_syntax::Tree`], read by a parser of colab's
//! own: pure Rust, lossless, error-tolerant. Step 3 of the move off
//! tree-sitter (docs/development-plan.md).
//!
//! `lexer.rs` reads Python's tokens and the offside rule's layout as
//! tree-sitter-python's scanner places it; `parser.rs` reads the grammar by
//! recursive descent into a tree whose kinds and fields are named as
//! `names.rs` says (tree-sitter-python's, for now). Comments and line
//! continuations are put in the deepest node with code on both sides of
//! them (a block reaches over the comments after it that are indented as
//! deep as it is); whitespace and newlines are trivia.

mod lexer;
mod names;
mod parser;

use std::sync::{Arc, OnceLock};

use colab_syntax::emit::{self, Names};
use colab_syntax::{BuildError, Grammar, SourceParser, Tree};

use parser::{Item, Tok};

/// colab's Python parser.
pub struct PythonParser;

/// The one parser; it holds nothing but its grammar.
pub static PARSER: PythonParser = PythonParser;

fn names() -> &'static Names {
    static NAMES: OnceLock<Names> = OnceLock::new();
    NAMES.get_or_init(|| {
        Names::new("python", names::NAMED, names::ANONYMOUS, names::FIELDS).with_text_leaves()
    })
}

colab_syntax::element!(Item, Tok, |n| named: n.named, extra: false);

/// `text` as a colab tree.
pub fn parse(text: &str) -> Result<Tree, BuildError> {
    let lexed = lexer::lex(text);
    let mut p = parser::Parser::new(text, lexed.tokens);
    let mut root = match p.module() {
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
        "module",
        &root.items,
        p.errors || lexed.errors,
    )
}

impl SourceParser for PythonParser {
    fn grammar(&self) -> Arc<Grammar> {
        names().grammar()
    }

    fn parse(&self, text: &str) -> Tree {
        parse(text).unwrap_or_else(|_| emit::fallback(text, names(), "module"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "import os\n\n# lead\ndef f(a, b=1):\n    # first\n    x = a + b  # trail\n    return f\"{x!r:>{w}}\"\n    # last\n# out\n";

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
        assert_eq!(parent("# lead"), Some("module"));
        assert_eq!(parent("# first"), Some("function_definition"));
        assert_eq!(parent("# trail"), Some("block"));
        assert_eq!(parent("# last"), Some("block"));
        assert_eq!(parent("# out"), Some("module"));
    }

    #[test]
    fn a_syntax_error_is_kept_and_the_rest_is_read() {
        let src = "def f():\n    x = = 1\n\ndef g():\n    pass\n";
        let tree = parse(src).unwrap();
        assert!(tree.has_error());
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, src);
        assert!(
            tree.nodes()
                .any(|n| n.kind_name() == "function_definition" && n.text().starts_with("def g"))
        );
    }
}
