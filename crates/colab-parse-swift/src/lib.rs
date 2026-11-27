//! Swift source as a [`colab_syntax::Tree`], read by a parser of colab's
//! own: pure Rust, lossless, error-tolerant. The last step of the move off
//! tree-sitter (docs/development-plan.md).
//!
//! `lexer.rs` reads the tokens, noting the line ends before them, and cuts
//! strings as tree-sitter-swift does; `parser.rs` reads tree-sitter-swift's
//! grammar by recursive descent (`parser/decl.rs` declarations,
//! `parser/expr.rs` expressions, `parser/types.rs` types and patterns),
//! ending statements where tree-sitter-swift's scanner does, into a tree
//! whose kinds and fields are named as `names.rs` says (tree-sitter's, for
//! now). Comments are put in the deepest node with code on both sides of
//! them; whitespace is trivia.

mod lexer;
mod names;
mod parser;

use std::sync::{Arc, OnceLock};

use colab_syntax::emit::{self, Names};
use colab_syntax::{BuildError, Grammar, SourceParser, Tree};

use parser::{Item, Tok};

/// colab's Swift parser.
pub struct SwiftParser;

/// The parser, for `Syntax::Own`.
pub static PARSER: SwiftParser = SwiftParser;

fn names() -> &'static Names {
    static NAMES: OnceLock<Names> = OnceLock::new();
    NAMES.get_or_init(|| Names::new("swift", names::NAMED, names::ANONYMOUS, names::FIELDS))
}

/// The named kind `name`; in a `const { … }`, an unknown name fails to
/// compile (`node.kind() == const { kind("import_spec") }`).
pub const fn kind(name: &str) -> colab_syntax::Kind {
    colab_syntax::emit::kind_in(names::NAMED, name)
}

colab_syntax::element!(Item, Tok, |n| named: !n.anonymous, extra: false);

/// `text`, Swift, as a colab tree.
pub fn parse(text: &str) -> Result<Tree, BuildError> {
    let lexed = lexer::lex(text);
    let comments = lexed.extras.iter().map(|&(s, e, _)| (s, e)).collect();
    let mut p = parser::Parser::new(text, lexed.tokens, comments);
    let mut root = match p.source_file() {
        Item::Node(n) => n,
        Item::Tok(_) => unreachable!("a file is a node"),
    };
    let extras = emit::comments(lexed.extras.iter().map(|&(start, end, block)| {
        let kind = if block {
            "multiline_comment"
        } else {
            "comment"
        };
        (start, end, kind)
    }));
    emit::place_extras(&mut root.items, extras);
    emit::build(
        text,
        names(),
        "source_file",
        &root.items,
        p.errors || lexed.errors,
    )
}

impl SourceParser for SwiftParser {
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

    const SRC: &str = "// lead\nimport Foundation\n\nstruct A: B {\n    let x: Int = 1 // trail\n    func f(y: Int) -> Int { x + y }\n}\n";

    #[test]
    fn a_parse_is_lossless_and_clean() {
        let tree = parse(SRC).unwrap();
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, SRC);
        assert!(!tree.has_error());
    }
}
