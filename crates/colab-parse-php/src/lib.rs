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

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use colab_syntax::{BuildError, Builder, Field, Flags, Grammar, Kind, SourceParser, Tree};

use parser::{Item, Node, Tok};

/// colab's PHP parser.
pub struct PhpParser;

/// The parser, for `Syntax::Own`.
pub static PARSER: PhpParser = PhpParser;

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
            grammar: Arc::new(Grammar::new("php", kinds, fields)),
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
        .map(|&(start, end)| comment(start, end))
        .collect();
    place_extras(&mut root, extras);
    let mut b = Builder::new(text, names().grammar.clone());
    if p.errors || lexed.errors {
        b.error_seen();
    }
    b.start_node(
        kind_of("program", true),
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

fn comment(start: usize, end: usize) -> Item {
    Item::Tok(Tok {
        kind: "comment",
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

impl SourceParser for PhpParser {
    fn grammar(&self) -> Arc<Grammar> {
        names().grammar.clone()
    }

    fn parse(&self, text: &str) -> Tree {
        parse(text).unwrap_or_else(|_| {
            let mut b = Builder::new(text, names().grammar.clone());
            let _ = b.start_node(kind_of("program", true), None, Flags::default());
            b.error_seen();
            b.finish()
                .unwrap_or_else(|_| unreachable!("a root over the text always builds"))
        })
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
