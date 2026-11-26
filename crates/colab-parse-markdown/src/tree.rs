//! The tree the parsers build, and its emission into colab's builder:
//! nodes, and tokens for the leaves (an `inline` span, a table cell, a
//! link destination). Bytes no token covers (whitespace, the text of a
//! fence's info string after its language) are trivia.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use colab_syntax::{BuildError, Builder, Field, Flags, Grammar, Kind, Tree};

use crate::names;

#[derive(Debug, Clone)]
pub(crate) enum Item {
    Tok {
        kind: &'static str,
        named: bool,
        range: std::ops::Range<usize>,
        field: Option<&'static str>,
    },
    Node {
        kind: &'static str,
        field: Option<&'static str>,
        items: Vec<Item>,
    },
    /// Bytes a node covers that no child is (a paragraph's final newline,
    /// the blank lines a list item ends with): trivia inside it.
    Gap(std::ops::Range<usize>),
}

impl Item {
    pub fn start(&self) -> usize {
        match self {
            Item::Tok { range, .. } | Item::Gap(range) => range.start,
            Item::Node { items, .. } => items.first().map_or(0, Item::start),
        }
    }

    pub fn end(&self) -> usize {
        match self {
            Item::Tok { range, .. } | Item::Gap(range) => range.end,
            Item::Node { items, .. } => items.last().map_or(0, Item::end),
        }
    }

    pub fn with(mut self, f: &'static str) -> Self {
        match &mut self {
            Item::Tok { field, .. } | Item::Node { field, .. } => *field = Some(f),
            Item::Gap(_) => {}
        }
        self
    }
}

/// A node over `items`, extended to `end` by a gap.
pub(crate) fn node_to(kind: &'static str, mut items: Vec<Item>, end: usize) -> Item {
    let last = items.last().map_or(end, Item::end);
    if end > last {
        items.push(Item::Gap(last..end));
    }
    node(kind, items)
}

/// A named leaf over `range`.
pub(crate) fn leaf(kind: &'static str, range: std::ops::Range<usize>) -> Item {
    Item::Tok {
        kind,
        named: true,
        range,
        field: None,
    }
}

/// Punctuation over `range`.
pub(crate) fn punct(kind: &'static str, range: std::ops::Range<usize>) -> Item {
    Item::Tok {
        kind,
        named: false,
        range,
        field: None,
    }
}

pub(crate) fn node(kind: &'static str, items: Vec<Item>) -> Item {
    Item::Node {
        kind,
        field: None,
        items,
    }
}

/// A node over `range` whose bytes are its own text, no children of
/// note: a leaf, as a token.
pub(crate) fn block(kind: &'static str, range: std::ops::Range<usize>) -> Item {
    leaf(kind, range)
}

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
            grammar: Arc::new(Grammar::new("markdown", kinds, fields)),
            kinds: ids,
        }
    })
}

pub(crate) fn grammar() -> Arc<Grammar> {
    names().grammar.clone()
}

fn kind_of(name: &'static str, named: bool) -> Kind {
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

/// `items` under a root of `root_kind` over all of `text`.
pub(crate) fn build(
    text: &str,
    root_kind: &'static str,
    items: &[Item],
) -> Result<Tree, BuildError> {
    let mut b = Builder::new(text, grammar());
    b.start_node(
        kind_of(root_kind, true),
        None,
        Flags {
            named: true,
            ..Flags::default()
        },
    )?;
    for item in items {
        emit(&mut b, item)?;
    }
    b.finish()
}

fn emit(b: &mut Builder, item: &Item) -> Result<(), BuildError> {
    let start = item.start();
    if start > b.pos() {
        b.trivia_to(start)?;
    }
    match item {
        Item::Gap(range) => {
            if range.end > b.pos() {
                b.trivia_to(range.end)?;
            }
            Ok(())
        }
        Item::Tok {
            kind,
            named,
            range,
            field,
        } => {
            if range.is_empty() {
                return Ok(());
            }
            let flags = Flags {
                named: *named,
                ..Flags::default()
            };
            b.token(kind_of(kind, *named), field_of(*field), flags, range.len())
        }
        Item::Node { kind, field, items } => {
            if items.is_empty() {
                return Ok(());
            }
            b.start_node(
                kind_of(kind, true),
                field_of(*field),
                Flags {
                    named: true,
                    ..Flags::default()
                },
            )?;
            for child in items {
                emit(b, child)?;
            }
            b.finish_node()
        }
    }
}
