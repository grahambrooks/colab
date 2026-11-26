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

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use colab_syntax::{BuildError, Builder, Field, Flags, Grammar, Kind, SourceParser, Tree};

pub use parser::Dialect;
use parser::{Item, Node, Tok};

/// colab's parser for one dialect: JavaScript, TypeScript or TSX.
pub struct JsParser(Dialect);

/// JavaScript (with JSX).
pub static PARSER: JsParser = JsParser(Dialect::Js);
/// TypeScript.
pub static TS_PARSER: JsParser = JsParser(Dialect::Ts);
/// TSX: TypeScript with JSX.
pub static TSX_PARSER: JsParser = JsParser(Dialect::Tsx);

struct Names {
    /// One grammar per dialect: the same kinds, named as tree-sitter's.
    grammars: [Arc<Grammar>; 3],
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
        let grammar = |name: &str| Arc::new(Grammar::new(name, kinds.clone(), fields.clone()));
        Names {
            grammars: [grammar("javascript"), grammar("typescript"), grammar("tsx")],
            kinds: ids,
        }
    })
}

fn grammar(dialect: Dialect) -> Arc<Grammar> {
    let k = match dialect {
        Dialect::Js => 0,
        Dialect::Ts => 1,
        Dialect::Tsx => 2,
    };
    names().grammars[k].clone()
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
    place_extras(&mut root, extras);
    let mut b = Builder::new(text, grammar(dialect));
    if p.errors || unclosed_comment {
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

/// Each comment (in order) into the deepest node holding code on both
/// sides of it: one pass down the tree, a binary search per comment and
/// level.
fn place_extras(node: &mut Node, extras: Vec<(usize, usize, &'static str)>) {
    if extras.is_empty() {
        return;
    }
    type Extras = Vec<(usize, usize, &'static str)>;
    let mut into: Vec<(usize, Extras)> = Vec::new();
    let mut here = Vec::new();
    for e in extras {
        let (start, end, _) = e;
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
        while let Some(&e) = pending.peek() {
            if e.1 > at {
                break;
            }
            items.push(extra(e));
            pending.next();
        }
        items.push(item);
    }
    items.extend(pending.map(extra));
    node.items = items;
}

fn extra((start, end, kind): (usize, usize, &'static str)) -> Item {
    Item::Tok(Tok {
        kind,
        named: true,
        start,
        end,
        field: None,
        missing: false,
        extra: true,
        hidden: false,
    })
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

impl SourceParser for JsParser {
    fn grammar(&self) -> Arc<Grammar> {
        grammar(self.0)
    }

    fn parse(&self, text: &str) -> Tree {
        parse_as(text, self.0).unwrap_or_else(|_| {
            let mut b = Builder::new(text, grammar(self.0));
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
