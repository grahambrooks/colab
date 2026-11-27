//! C# source as a [`colab_syntax::Tree`], read by a parser of colab's own:
//! pure Rust, lossless, error-tolerant. Step 3 of the move off
//! tree-sitter (docs/development-plan.md).
//!
//! `lexer.rs` reads C#'s tokens, interpolated strings cut as
//! tree-sitter-c-sharp's scanner cuts them; `parser.rs` reads
//! tree-sitter-c-sharp's grammar by recursive descent (`parser/types.rs`
//! types and names, `parser/expr.rs` expressions, patterns and queries)
//! into a tree whose kinds and fields are named as `names.rs` says
//! (tree-sitter-c-sharp's, for now). Comments and the directives other
//! than `#if` are extras, put in the deepest node with code on both sides
//! of them; whitespace is trivia.

mod lexer;
mod names;
mod parser;

use std::sync::{Arc, OnceLock};

use colab_syntax::emit::{self, Element as _, Names};
use colab_syntax::{BuildError, Grammar, SourceParser, Tree};

use parser::{Item, Tok};

/// colab's C# parser.
pub struct CSharpParser;

/// The one parser; it holds nothing but its grammar.
pub static PARSER: CSharpParser = CSharpParser;

fn names() -> &'static Names {
    static NAMES: OnceLock<Names> = OnceLock::new();
    NAMES.get_or_init(|| Names::new("c_sharp", names::NAMED, names::ANONYMOUS, names::FIELDS))
}

colab_syntax::element!(Item, Tok, |n| named: true, extra: n.extra);

/// `text` as a colab tree.
pub fn parse(text: &str) -> Result<Tree, BuildError> {
    let lexed = lexer::lex(text);
    let mut p = parser::Parser::new(text, lexed.tokens);
    let mut root = match p.program() {
        Item::Node(n) => n,
        Item::Tok(_) => unreachable!("a file is a node"),
    };
    // Comments inside a directive's line go into its node; the rest, and
    // the directives, into the tree.
    let mut comments: Vec<Item> = lexed
        .extras
        .iter()
        .map(|e| Item::extra(e.start, e.end, "comment"))
        .collect();
    let mut extras = std::mem::take(&mut p.extras);
    if !extras.is_empty() && !comments.is_empty() {
        let mut rest = Vec::with_capacity(comments.len());
        let mut d = 0;
        for c in comments {
            while d < extras.len() && extras[d].end() <= c.start() {
                d += 1;
            }
            match extras.get_mut(d) {
                Some(Item::Node(n)) if n.items.first().map_or(0, Item::start) < c.start() => {
                    let at = n.items.partition_point(|i| i.start() < c.start());
                    n.items.insert(at, c);
                }
                _ => rest.push(c),
            }
        }
        comments = rest;
    }
    extras.extend(comments);
    extras.sort_by_key(Item::start);
    emit::place_extras(&mut root.items, extras);
    emit::build(
        text,
        names(),
        "compilation_unit",
        &root.items,
        p.errors || lexed.errors,
    )
}

impl SourceParser for CSharpParser {
    fn grammar(&self) -> Arc<Grammar> {
        names().grammar()
    }

    fn parse(&self, text: &str) -> Tree {
        parse(text).unwrap_or_else(|_| emit::fallback(text, names(), "compilation_unit"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "// lead\nusing System;\n\nnamespace N;\n\npublic class A {\n    /// doc\n    int F(int x) {\n        return x >> 1; // trail\n    }\n}\n";

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
        assert_eq!(parent("// lead"), Some("compilation_unit"));
        assert_eq!(parent("/// doc"), Some("declaration_list"));
        assert_eq!(parent("// trail"), Some("block"));
    }

    #[test]
    fn a_syntax_error_is_kept_and_the_rest_is_read() {
        let src = "class A {\n    void F() { int x = ; }\n    void G() {}\n}\n";
        let tree = parse(src).unwrap();
        assert!(tree.has_error());
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, src);
        assert!(
            tree.nodes()
                .any(|n| n.kind_name() == "method_declaration" && n.text() == "void G() {}")
        );
    }
}
