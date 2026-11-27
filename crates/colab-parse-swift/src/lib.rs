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

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use colab_syntax::{BuildError, Builder, Field, Flags, Grammar, Kind, SourceParser, Tree};

use parser::{Item, Node, Tok};

/// colab's Swift parser.
pub struct SwiftParser;

/// The parser, for `Syntax::Own`.
pub static PARSER: SwiftParser = SwiftParser;

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
        let fields: Vec<Box<str>> = names::FIELDS.iter().map(|f| (*f).into()).collect();
        Names {
            grammar: Arc::new(Grammar::new("swift", kinds, fields)),
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

/// `text`, Swift, as a colab tree.
pub fn parse(text: &str) -> Result<Tree, BuildError> {
    let lexed = lexer::lex(text);
    let comments = lexed.extras.iter().map(|&(s, e, _)| (s, e)).collect();
    let mut p = parser::Parser::new(text, lexed.tokens, comments);
    let mut root = match p.source_file() {
        Item::Node(n) => n,
        Item::Tok(_) => unreachable!("a file is a node"),
    };
    let extras: Vec<Item> = lexed
        .extras
        .iter()
        .map(|&(start, end, block)| comment(start, end, block))
        .collect();
    place_extras(&mut root, extras);
    let mut b = Builder::new(text, names().grammar.clone());
    if p.errors || lexed.errors {
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

fn comment(start: usize, end: usize, block: bool) -> Item {
    Item::Tok(Tok {
        kind: if block {
            "multiline_comment"
        } else {
            "comment"
        },
        named: true,
        start,
        end,
        field: None,
        hidden: false,
        missing: false,
        extra: true,
    })
}

/// Each comment (in order) into the deepest node holding code on both
/// sides of it.
fn place_extras(node: &mut Node, extras: Vec<Item>) {
    if extras.is_empty() {
        return;
    }
    let mut into: Vec<(usize, Vec<Item>)> = Vec::new();
    let mut here = Vec::new();
    for e in extras {
        let (start, end) = (e.start(), e.end());
        let k = node.items.partition_point(|i| i.end() < end);
        match node.items.get(k) {
            Some(i @ Item::Node(n))
                if !n.items.is_empty() && i.start() < start && end <= i.end() =>
            {
                match into.last_mut() {
                    Some((last, list)) if *last == k => list.push(e),
                    _ => into.push((k, vec![e])),
                }
            }
            _ => here.push(e),
        }
    }
    for (k, list) in into {
        if let Item::Node(n) = &mut node.items[k] {
            place_extras(n, list);
        }
    }
    if here.is_empty() {
        return;
    }
    let old = std::mem::take(&mut node.items);
    let mut items = Vec::with_capacity(old.len() + here.len());
    let mut pending = here.into_iter().peekable();
    for item in old {
        let at = item.start();
        while let Some(e) = pending.peek() {
            if e.end() > at {
                break;
            }
            if let Some(e) = pending.next() {
                items.push(e);
            }
        }
        items.push(item);
    }
    items.extend(pending);
    node.items = items;
}

fn emit(b: &mut Builder, item: &Item) -> Result<(), BuildError> {
    let start = item.start();
    if start > b.pos() {
        b.trivia_to(start)?;
    }
    match item {
        Item::Tok(t) if t.hidden => {
            if t.end > b.pos() {
                b.trivia_to(t.end)?;
            }
            Ok(())
        }
        // Text already passed as trivia (a comment after a statement end
        // placed past it, in a file read with errors).
        Item::Tok(t) if t.start < b.pos() && t.end > t.start => Ok(()),
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
        Item::Node(n) if n.kind.is_empty() => {
            for child in &n.items {
                emit(b, child)?;
            }
            Ok(())
        }
        Item::Node(n) => {
            if n.items.is_empty() {
                return Ok(());
            }
            let named = !n.anonymous;
            b.start_node(
                kind_of(n.kind, named),
                field_of(n.field),
                Flags {
                    named,
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

impl SourceParser for SwiftParser {
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

    const SRC: &str = "// lead\nimport Foundation\n\nstruct A: B {\n    let x: Int = 1 // trail\n    func f(y: Int) -> Int { x + y }\n}\n";

    #[test]
    fn a_parse_is_lossless_and_clean() {
        let tree = parse(SRC).unwrap();
        let text: String = tree.tokens().map(|t| t.text()).collect();
        assert_eq!(text, SRC);
        assert!(!tree.has_error());
    }
}
