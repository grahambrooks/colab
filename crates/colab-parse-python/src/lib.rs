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

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use colab_syntax::{BuildError, Builder, Field, Flags, Grammar, Kind, SourceParser, Tree};

use parser::{Item, Node, Tok};

/// colab's Python parser.
pub struct PythonParser;

/// The one parser; it holds nothing but its grammar.
pub static PARSER: PythonParser = PythonParser;

struct Names {
    grammar: Arc<Grammar>,
    kinds: HashMap<(&'static str, bool), Kind>,
}

fn names() -> &'static Names {
    static NAMES: OnceLock<Names> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut kinds = Vec::new();
        let mut ids = HashMap::new();
        for (list, named) in [(names::NAMED, true), (names::ANONYMOUS, false)] {
            for name in list {
                let id = u16::try_from(kinds.len()).unwrap_or(u16::MAX);
                ids.insert((*name, named), Kind(id));
                kinds.push(((*name).into(), named));
            }
        }
        let fields = names::FIELDS.iter().map(|f| (*f).into()).collect();
        Names {
            grammar: Arc::new(Grammar::new("python", kinds, fields)),
            kinds: ids,
        }
    })
}

fn kind_of(name: &'static str, named: bool) -> Kind {
    if name == "ERROR" {
        return Kind::ERROR;
    }
    names()
        .kinds
        .get(&(name, named))
        .copied()
        .unwrap_or(Kind::ERROR)
}

fn field_of(name: Option<&'static str>) -> Option<Field> {
    let name = name?;
    let i = names::FIELDS.iter().position(|f| *f == name)?;
    u16::try_from(i).ok().map(Field)
}

/// `text` as a colab tree.
pub fn parse(text: &str) -> Result<Tree, BuildError> {
    let lexed = lexer::lex(text);
    let mut p = parser::Parser::new(text, lexed.tokens);
    let mut root = match p.module() {
        Item::Node(n) => n,
        Item::Tok(_) => unreachable!("a file is a node"),
    };
    for extra in &lexed.extras {
        place_extra(&mut root, extra.kind, extra.start, extra.end);
    }
    let mut b = Builder::new(text, names().grammar.clone());
    if p.errors || lexed.errors {
        b.error_seen();
    }
    b.start_node(
        kind_of("module", true),
        None,
        Flags {
            named: true,
            ..Flags::default()
        },
    )?;
    for item in &root.items {
        emit(&mut b, item)?;
    }
    b.finish()
}

/// A comment or line continuation into the deepest node holding code on
/// both sides of it.
fn place_extra(root: &mut Node, kind: &'static str, start: usize, end: usize) {
    let mut node = root;
    loop {
        let inside = node.items.iter().position(|i| {
            matches!(i, Item::Node(n) if !n.items.is_empty()) && i.start() < start && end <= i.end()
        });
        match inside {
            Some(k) => match &mut node.items[k] {
                Item::Node(n) => node = n,
                Item::Tok(_) => unreachable!(),
            },
            None => break,
        }
    }
    let at = node
        .items
        .iter()
        .position(|i| i.start() >= end)
        .unwrap_or(node.items.len());
    node.items.insert(
        at,
        Item::Tok(Tok {
            kind,
            named: true,
            start,
            end,
            field: None,
            hidden: false,
            missing: false,
            extra: true,
        }),
    );
}

fn emit(b: &mut Builder, item: &Item) -> Result<(), BuildError> {
    let start = item.start();
    if start > b.pos() {
        b.trivia_to(start)?;
    }
    match item {
        Item::Tok(t) if t.hidden => b.trivia_to(t.end),
        Item::Tok(t) => {
            let flags = Flags {
                named: t.named,
                missing: t.missing,
                extra: t.extra,
                ..Flags::default()
            };
            b.token(
                kind_of(t.kind, t.named),
                field_of(t.field),
                flags,
                t.end - t.start,
            )
        }
        Item::Node(n) => {
            if n.items.is_empty() {
                return Ok(());
            }
            // A node of nothing but text (a string's content without
            // escapes) is a leaf, as tree-sitter's hidden tokens left it:
            // its text is what a pattern compares.
            let len = item.end() - start;
            if len > 0
                && n.items
                    .iter()
                    .all(|i| matches!(i, Item::Tok(t) if t.hidden))
            {
                let flags = Flags {
                    named: n.named,
                    ..Flags::default()
                };
                return b.token(kind_of(n.kind, n.named), field_of(n.field), flags, len);
            }
            b.start_node(
                kind_of(n.kind, n.named),
                field_of(n.field),
                Flags {
                    named: n.named,
                    ..Flags::default()
                },
            )?;
            for child in &n.items {
                emit(b, child)?;
            }
            b.finish_node()
        }
    }
}

impl SourceParser for PythonParser {
    fn grammar(&self) -> Arc<Grammar> {
        names().grammar.clone()
    }

    fn parse(&self, text: &str) -> Tree {
        parse(text).unwrap_or_else(|_| {
            let mut b = Builder::new(text, names().grammar.clone());
            let _ = b.start_node(kind_of("module", true), None, Flags::default());
            b.error_seen();
            b.finish()
                .unwrap_or_else(|_| unreachable!("a root over the text always builds"))
        })
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
