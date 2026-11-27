//! JavaScript (with JSX), TypeScript and TSX as [`colab_syntax::Tree`]s,
//! read by a parser of colab's own: pure Rust, lossless, error-tolerant.
//! Step 3 of the move off tree-sitter (docs/development-plan.md).
//!
//! `lexer.rs` lexes on demand, in the mode the parser is in (a regex or a
//! division, JSX text, a template's text); `parser.rs` reads
//! tree-sitter-javascript's grammar by recursive descent, and
//! `parser/ts.rs` tree-sitter-typescript's additions for the TypeScript
//! dialects, kinds and fields named as `names.rs` says. Comments are put
//! in the deepest node with code on both sides of them; whitespace is
//! trivia.

mod lexer;
mod names;
mod parser;

use std::sync::{Arc, OnceLock};

use colab_syntax::emit::{self, Names};
use colab_syntax::{BuildError, Grammar, SourceParser, Tree};

pub use parser::Dialect;
use parser::{Item, Tok};

/// colab's parser for one dialect: JavaScript, TypeScript or TSX.
pub struct JsParser(Dialect);

/// JavaScript (with JSX).
pub static PARSER: JsParser = JsParser(Dialect::Js);
/// TypeScript.
pub static TS_PARSER: JsParser = JsParser(Dialect::Ts);
/// TSX: TypeScript with JSX.
pub static TSX_PARSER: JsParser = JsParser(Dialect::Tsx);

/// One names table per dialect: the same kinds, named as tree-sitter's.
fn names_of(dialect: Dialect) -> &'static Names {
    static NAMES: [OnceLock<Names>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let (i, lang) = match dialect {
        Dialect::Js => (0, "javascript"),
        Dialect::Ts => (1, "typescript"),
        Dialect::Tsx => (2, "tsx"),
    };
    NAMES[i].get_or_init(|| Names::new(lang, names::NAMED, names::ANONYMOUS, names::FIELDS))
}

colab_syntax::element!(Item, Tok, |n| named: true, extra: false);

/// `text`, JavaScript, as a colab tree.
pub fn parse(text: &str) -> Result<Tree, BuildError> {
    parse_as(text, Dialect::Js)
}

/// `text` in `dialect` as a colab tree.
pub fn parse_as(text: &str, dialect: Dialect) -> Result<Tree, BuildError> {
    let mut p = parser::Parser::new(text, dialect);
    let mut root = match p.program() {
        Item::Node(n) => n,
        Item::Tok(_) => unreachable!("a file is a node"),
    };
    let mut extras: Vec<(usize, usize, &'static str)> =
        p.extras.iter().map(|e| (e.start, e.end, e.kind)).collect();
    extras.sort_unstable();
    extras.dedup();
    // The lexer's own errors may be in tokens lexed ahead and then lexed
    // again (JSX text read as code): only what the parser kept counts
    // (its missing closers, its `ERROR`s), and an unclosed comment.
    let unclosed_comment = extras
        .iter()
        .any(|(s, e, _)| text[*s..*e].starts_with("/*") && !text[*s..*e].ends_with("*/"));
    emit::place_extras(&mut root.items, emit::comments(extras));
    emit::build(
        text,
        names_of(dialect),
        "program",
        &root.items,
        p.errors || unclosed_comment,
    )
}

impl SourceParser for JsParser {
    fn grammar(&self) -> Arc<Grammar> {
        names_of(self.0).grammar()
    }

    fn parse(&self, text: &str) -> Tree {
        parse_as(text, self.0).unwrap_or_else(|_| emit::fallback(text, names_of(self.0), "program"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "// lead\nimport a from \"m\";\nconst re = /a\\/b/g, x = a / 2;\nfunction f() {\n  return <div className=\"x\">hi {name}</div> // trail\n}\n";

    #[test]
    fn a_parse_is_lossless_and_clean() {
        let tree = parse(SRC).unwrap();
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, SRC);
        assert!(!tree.has_error());
        assert!(tree.nodes().any(|n| n.kind_name() == "regex"));
        assert!(
            tree.nodes()
                .any(|n| n.kind_name() == "jsx_text" && n.text() == "hi ")
        );
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
        assert_eq!(parent("// trail"), Some("statement_block"));
    }
}
