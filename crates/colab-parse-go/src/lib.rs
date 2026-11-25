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

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use colab_syntax::{BuildError, Builder, Field, Flags, Grammar, Kind, SourceParser, Tree};

use parser::{Item, Node, Tok};

/// colab's Go parser.
pub struct GoParser;

/// The one parser; it holds nothing but its grammar.
pub static PARSER: GoParser = GoParser;

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
            grammar: Arc::new(Grammar::new("go", kinds, fields)),
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
    let (tokens, comments) = lexer::lex(text);
    let mut p = parser::Parser::new(text, tokens);
    let mut root = match p.source_file() {
        Item::Node(n) => n,
        Item::Tok(_) => unreachable!("a file is a node"),
    };
    for (start, end) in comments {
        place_comment(&mut root, start, end);
    }
    let mut b = Builder::new(text, names().grammar.clone());
    if p.errors {
        b.error_seen();
    }
    b.start_node(
        kind_of("source_file", true),
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

/// `comment` into the deepest node holding code on both sides of it.
fn place_comment(root: &mut Node, start: usize, end: usize) {
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
            kind: "comment",
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
            b.start_node(
                kind_of(n.kind, true),
                field_of(n.field),
                Flags {
                    named: true,
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

impl SourceParser for GoParser {
    fn grammar(&self) -> Arc<Grammar> {
        names().grammar.clone()
    }

    fn parse(&self, text: &str) -> Tree {
        parse(text).unwrap_or_else(|_| {
            let mut b = Builder::new(text, names().grammar.clone());
            let _ = b.start_node(kind_of("source_file", true), None, Flags::default());
            b.error_seen();
            b.finish()
                .unwrap_or_else(|_| unreachable!("a root over the text always builds"))
        })
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
