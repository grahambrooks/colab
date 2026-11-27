//! C++ (and, as a dialect, C) source as a [`colab_syntax::Tree`], read by a
//! parser of colab's own: pure Rust, lossless, error-tolerant. Step 3 of the
//! move off tree-sitter (docs/development-plan.md).
//!
//! `lexer.rs` reads the tokens and the preprocessor's lines; `parser.rs`
//! reads tree-sitter-cpp's grammar by recursive descent (`parser/decl.rs`
//! declarations, declarators, classes and templates, `parser/expr.rs`
//! expressions) into a tree whose kinds and fields are named as `names.rs`
//! says (tree-sitter-cpp's, for now). Comments are put in the deepest node
//! with code on both sides of them; whitespace is trivia.

mod lexer;
mod names;
mod parser;

use std::sync::{Arc, OnceLock};

use colab_syntax::emit::{self, Element as _, Names};
use colab_syntax::{BuildError, Grammar, SourceParser, Tree};

pub use parser::Dialect;
use parser::{Item, Tok};

/// colab's parser for one dialect.
pub struct CParser(Dialect);

/// C++.
pub static CPP_PARSER: CParser = CParser(Dialect::Cpp);
/// C.
pub static C_PARSER: CParser = CParser(Dialect::C);

/// C++'s names or C's: the same kinds, named as tree-sitter's.
fn names_of(dialect: Dialect) -> &'static Names {
    static NAMES: [OnceLock<Names>; 2] = [OnceLock::new(), OnceLock::new()];
    let (i, lang) = match dialect {
        Dialect::Cpp => (0, "cpp"),
        Dialect::C => (1, "c"),
    };
    NAMES[i].get_or_init(|| Names::new(lang, names::NAMED, names::ANONYMOUS, names::FIELDS))
}

colab_syntax::element!(Item, Tok, |n| named: true, extra: false);

/// `text`, C++, as a colab tree.
pub fn parse(text: &str) -> Result<Tree, BuildError> {
    parse_as(text, Dialect::Cpp)
}

/// `text` in `dialect` as a colab tree.
pub fn parse_as(text: &str, dialect: Dialect) -> Result<Tree, BuildError> {
    let lexed = lexer::lex(text, dialect == Dialect::C);
    let mut p = parser::Parser::new(text, lexed.tokens, dialect);
    let mut root = match p.program() {
        Item::Node(n) => n,
        Item::Tok(_) => unreachable!("a file is a node"),
    };
    let extras: Vec<Item> = lexed
        .extras
        .iter()
        .map(|e| Item::extra(e.start, e.end, "comment"))
        .collect();
    emit::place_extras(&mut root.items, extras);
    emit::build(
        text,
        names_of(dialect),
        "translation_unit",
        &root.items,
        p.errors || lexed.errors,
    )
}

impl SourceParser for CParser {
    fn grammar(&self) -> Arc<Grammar> {
        names_of(self.0).grammar()
    }

    fn parse(&self, text: &str) -> Tree {
        parse_as(text, self.0)
            .unwrap_or_else(|_| emit::fallback(text, names_of(self.0), "translation_unit"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "// lead\n#include <vector>\n\nnamespace n {\nclass A {\n public:\n  int f(int x) const { return x >> 1; } // trail\n};\n}  // namespace n\n";

    #[test]
    fn a_parse_is_lossless_and_clean() {
        let tree = parse(SRC).unwrap();
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, SRC);
        assert!(!tree.has_error());
    }

    #[test]
    fn a_syntax_error_is_kept_and_the_rest_is_read() {
        let src = "int f() { int x = ; }\nint g() { return 1; }\n";
        let tree = parse(src).unwrap();
        assert!(tree.has_error());
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, src);
        assert!(
            tree.nodes()
                .any(|n| n.kind_name() == "function_definition" && n.text().starts_with("int g"))
        );
    }
}
