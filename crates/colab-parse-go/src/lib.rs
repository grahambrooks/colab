//! Go source as a [`colab_syntax::Tree`], read by a parser of colab's own:
//! pure Rust, lossless, error-tolerant. Step 3 of the move off
//! tree-sitter (docs/development-plan.md).
//!
//! `lexer.rs` reads Go's tokens by the language specification, inserting
//! the semicolons the specification inserts at line ends; `parser.rs` reads
//! the grammar by recursive descent into a tree whose kinds and fields are
//! named as `names.rs` says (tree-sitter-go's, for now). Comments are put
//! in the deepest node with code on both sides of them; whitespace,
//! including a statement's ending newline, is trivia.

mod lexer;
mod names;
mod parser;

use std::sync::{Arc, OnceLock};

use colab_syntax::emit::{self, Names};
use colab_syntax::{BuildError, Grammar, SourceParser, Tree};

use parser::{Item, Tok};

/// colab's Go parser.
pub struct GoParser;

/// The one parser; it holds nothing but its grammar.
pub static PARSER: GoParser = GoParser;

fn names() -> &'static Names {
    static NAMES: OnceLock<Names> = OnceLock::new();
    NAMES.get_or_init(|| Names::new("go", names::NAMED, names::ANONYMOUS, names::FIELDS))
}

colab_syntax::element!(Item, Tok, |n| named: true, extra: false);

/// `text` as a colab tree.
pub fn parse(text: &str) -> Result<Tree, BuildError> {
    let (tokens, comments) = lexer::lex(text);
    let mut p = parser::Parser::new(text, tokens);
    let mut root = match p.source_file() {
        Item::Node(n) => n,
        Item::Tok(_) => unreachable!("a file is a node"),
    };
    let comments = emit::comments(
        comments
            .into_iter()
            .map(|(start, end)| (start, end, "comment")),
    );
    emit::place_extras(&mut root.items, comments);
    emit::build(text, names(), "source_file", &root.items, p.errors)
}

impl SourceParser for GoParser {
    fn grammar(&self) -> Arc<Grammar> {
        names().grammar()
    }

    fn parse(&self, text: &str) -> Tree {
        parse(text).unwrap_or_else(|_| emit::fallback(text, names(), "source_file"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "package p\n\n// lead\nconst a, b = 1, 2\n\nfunc f() {\n\t// first\n\tx := 1 // trail\n\ty++\n\t// last\n}\n";

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
        assert_eq!(parent("// lead"), Some("source_file"));
        assert_eq!(parent("// first"), Some("block"));
        assert_eq!(parent("// trail"), Some("statement_list"));
        assert_eq!(parent("// last"), Some("block"));
    }

    #[test]
    fn a_syntax_error_is_kept_and_the_rest_is_read() {
        let src = "package p\n\nfunc f() {\n\tx := (\n}\n\nfunc g() {}\n";
        let tree = parse(src).unwrap();
        assert!(tree.has_error());
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, src);
        assert!(
            tree.nodes()
                .any(|n| n.kind_name() == "function_declaration" && n.text() == "func g() {}")
        );
    }
}
