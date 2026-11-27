//! PHP source as a [`colab_syntax::Tree`], read by a parser of colab's own:
//! pure Rust, lossless, error-tolerant. Step 3 of the move off tree-sitter
//! (docs/development-plan.md).
//!
//! `lexer.rs` reads the tokens, cutting strings, heredocs and nowdocs where
//! tree-sitter-php's scanner does; `parser.rs` reads tree-sitter-php's
//! `php_only` grammar by recursive descent (`parser/decl.rs` declarations,
//! types and parameters, `parser/expr.rs` expressions and strings) into a
//! tree whose kinds and fields are named as `names.rs` says
//! (tree-sitter-php's, for now). Comments are put in the deepest node with
//! code on both sides of them; whitespace is trivia.

mod lexer;
mod names;
mod parser;

use std::sync::{Arc, OnceLock};

use colab_syntax::emit::{self, Element as _, Names};
use colab_syntax::{BuildError, Grammar, SourceParser, Tree};

use parser::{Item, Tok};

/// colab's PHP parser.
pub struct PhpParser;

/// The parser, for `Syntax::Own`.
pub static PARSER: PhpParser = PhpParser;

fn names() -> &'static Names {
    static NAMES: OnceLock<Names> = OnceLock::new();
    NAMES.get_or_init(|| Names::new("php", names::NAMED, names::ANONYMOUS, names::FIELDS))
}

colab_syntax::element!(Item, Tok, |n| named: true, extra: false);

/// `text`, PHP, as a colab tree.
pub fn parse(text: &str) -> Result<Tree, BuildError> {
    let lexed = lexer::lex(text);
    let mut p = parser::Parser::new(text, lexed.tokens);
    let mut root = match p.program() {
        Item::Node(n) => n,
        Item::Tok(_) => unreachable!("a file is a node"),
    };
    let extras: Vec<Item> = lexed
        .extras
        .iter()
        .map(|&(start, end)| Item::extra(start, end, "comment"))
        .collect();
    emit::place_extras(&mut root.items, extras);
    emit::build(
        text,
        names(),
        "program",
        &root.items,
        p.errors || lexed.errors,
    )
}

impl SourceParser for PhpParser {
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

    const SRC: &str = "<?php\n// lead\nnamespace App;\n\nuse Foo\\Bar;\n\nfinal class A extends B {\n    public function f(int $x): int { return $x + 1; } // trail\n}\n";

    #[test]
    fn a_parse_is_lossless_and_clean() {
        let tree = parse(SRC).unwrap();
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, SRC);
        assert!(!tree.has_error());
    }

    #[test]
    fn a_syntax_error_is_kept_and_the_rest_is_read() {
        let src = "<?php\nfunction f() { $x = ; }\nfunction g() { return 1; }\n";
        let tree = parse(src).unwrap();
        assert!(tree.has_error());
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, src);
        assert!(
            tree.nodes()
                .any(|n| n.kind_name() == "function_definition"
                    && n.text().starts_with("function g"))
        );
    }
}
