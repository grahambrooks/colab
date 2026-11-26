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

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use colab_syntax::{BuildError, Builder, Field, Flags, Grammar, Kind, SourceParser, Tree};

use parser::{Item, Node, Tok};

/// colab's C# parser.
pub struct CSharpParser;

/// The one parser; it holds nothing but its grammar.
pub static PARSER: CSharpParser = CSharpParser;

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
            grammar: Arc::new(Grammar::new("c_sharp", kinds, fields)),
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
    let mut root = match p.program() {
        Item::Node(n) => n,
        Item::Tok(_) => unreachable!("a file is a node"),
    };
    // Comments inside a directive's line go into its node; the rest, and
    // the directives, into the tree.
    let mut comments: Vec<Item> = lexed
        .extras
        .iter()
        .map(|e| comment(e.start, e.end))
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
    place_extras(&mut root, extras);
    let mut b = Builder::new(text, names().grammar.clone());
    if p.errors || lexed.errors {
        b.error_seen();
    }
    b.start_node(
        kind_of("compilation_unit", true),
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

/// Each extra (in order) into the deepest node holding code on both sides
/// of it: one pass down the tree, a binary search per extra and level.
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
                if !n.items.is_empty() && !n.extra && i.start() < start && end <= i.end() =>
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
                    extra: n.extra,
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

impl SourceParser for CSharpParser {
    fn grammar(&self) -> Arc<Grammar> {
        names().grammar.clone()
    }

    fn parse(&self, text: &str) -> Tree {
        parse(text).unwrap_or_else(|_| {
            let mut b = Builder::new(text, names().grammar.clone());
            let _ = b.start_node(kind_of("compilation_unit", true), None, Flags::default());
            b.error_seen();
            b.finish()
                .unwrap_or_else(|_| unreachable!("a root over the text always builds"))
        })
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
