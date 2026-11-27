//! What every parser of colab's own shares between its parse and the
//! [`Tree`]: the names table ([`Names`]), comment placement
//! ([`place_extras`]) and emission through the [`Builder`] ([`build`]).
//!
//! A parser keeps its own item tree (its `Item`, with the field rules its
//! grammar needs) and shows it to this module through [`Element`]: a token
//! or a node, its kind, field and flags. A node of kind `""` is
//! transparent: its children are its parent's.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{BuildError, Builder, Field, Flags, Grammar, Kind, Tree};

/// A token of a parser's item tree.
#[derive(Debug, Clone, Copy)]
pub struct TokenView {
    pub kind: &'static str,
    pub named: bool,
    pub field: Option<&'static str>,
    pub start: usize,
    pub end: usize,
    /// Text inside the node it is in, no token of its own (a statement's
    /// zero-width end, a separator the grammar hides).
    pub hidden: bool,
    pub missing: bool,
    pub extra: bool,
}

/// A node of a parser's item tree.
#[derive(Debug, Clone, Copy)]
pub struct NodeView<'a, E> {
    /// `""` for a hidden rule, whose children are its parent's.
    pub kind: &'static str,
    pub named: bool,
    pub field: Option<&'static str>,
    /// An extra node (C#'s preprocessor directives), which takes no
    /// comment inside it.
    pub extra: bool,
    pub children: &'a [E],
}

/// One item: a token or a node.
#[derive(Debug, Clone, Copy)]
pub enum View<'a, E> {
    Token(TokenView),
    Node(NodeView<'a, E>),
}

/// A parser's item, as this module reads it.
pub trait Element: Sized {
    fn view(&self) -> View<'_, Self>;
    /// A node's children, to place comments among; `None` for a token.
    fn children_mut(&mut self) -> Option<&mut Vec<Self>>;
    /// The extra token of a comment (`kind`) over `start..end`.
    fn extra(start: usize, end: usize, kind: &'static str) -> Self;

    /// Where the item starts: a node at its first child (0 when empty).
    fn start(&self) -> usize {
        match self.view() {
            View::Token(t) => t.start,
            View::Node(n) => n.children.first().map_or(0, Element::start),
        }
    }

    /// Where the item ends: a node at its last child (0 when empty).
    fn end(&self) -> usize {
        match self.view() {
            View::Token(t) => t.end,
            View::Node(n) => n.children.last().map_or(0, Element::end),
        }
    }
}

/// A language's kind and field names, from its `names.rs` lists, and the
/// [`Grammar`] they make.
#[derive(Debug)]
pub struct Names {
    grammar: Arc<Grammar>,
    kinds: HashMap<(&'static str, bool), Kind>,
    fields: &'static [&'static str],
    text_leaves: bool,
}

impl Names {
    /// The names of `lang`: its named kinds, its anonymous kinds (tokens
    /// named by their text) and its fields, ids in that order.
    pub fn new(
        lang: &str,
        named: &[&'static str],
        anonymous: &[&'static str],
        fields: &'static [&'static str],
    ) -> Self {
        let mut list = Vec::new();
        let mut kinds = HashMap::new();
        for (names, is_named) in [(named, true), (anonymous, false)] {
            for name in names {
                let id = u16::try_from(list.len()).unwrap_or(u16::MAX);
                kinds.insert((*name, is_named), Kind(id));
                list.push(((*name).into(), is_named));
            }
        }
        let field_names: Vec<Box<str>> = fields.iter().map(|f| (*f).into()).collect();
        Names {
            grammar: Arc::new(Grammar::new(lang, list, field_names)),
            kinds,
            fields,
            text_leaves: false,
        }
    }

    /// A node of nothing but hidden text (Python's string content without
    /// escapes) is emitted as a leaf token, as tree-sitter's hidden tokens
    /// left it: its text is what a pattern compares.
    pub fn with_text_leaves(mut self) -> Self {
        self.text_leaves = true;
        self
    }

    pub fn grammar(&self) -> Arc<Grammar> {
        self.grammar.clone()
    }

    /// The kind `name` (named or not); `ERROR` and an unknown name are
    /// [`Kind::ERROR`].
    pub fn kind(&self, name: &str, named: bool) -> Kind {
        if name == "ERROR" {
            return Kind::ERROR;
        }
        self.kinds
            .get(&(name, named))
            .copied()
            .unwrap_or(Kind::ERROR)
    }

    /// The field `name`, if the language has it.
    pub fn field(&self, name: Option<&str>) -> Option<Field> {
        let name = name?;
        let i = self.fields.iter().position(|f| *f == name)?;
        u16::try_from(i).ok().map(Field)
    }
}

/// The comments `(start, end, kind)` as extra tokens, for
/// [`place_extras`].
pub fn comments<E: Element>(
    list: impl IntoIterator<Item = (usize, usize, &'static str)>,
) -> Vec<E> {
    list.into_iter()
        .map(|(start, end, kind)| E::extra(start, end, kind))
        .collect()
}

/// Each of `extras` (comments, as [`comments`] makes them, or a parser's
/// extra nodes) into the deepest node of `items` holding code on both
/// sides of it, else among `items` before the first item after it.
pub fn place_extras<E: Element>(items: &mut Vec<E>, mut extras: Vec<E>) {
    if extras.is_empty() {
        return;
    }
    // A node's range is its first and last child's, so an item out of
    // order anywhere below makes an ancestor's range wrong: put the whole
    // tree in order first.
    in_order_all(items);
    extras.sort_by_key(E::start);
    place(items, extras);
}

/// Every node's children of `items`, and `items`, in order ([`in_order`]).
fn in_order_all<E: Element>(items: &mut [E]) {
    for item in items.iter_mut() {
        if let Some(children) = item.children_mut() {
            in_order_all(children);
        }
    }
    in_order(items);
}

fn place<E: Element>(items: &mut Vec<E>, extras: Vec<E>) {
    let mut into: Vec<(usize, Vec<E>)> = Vec::new();
    let mut here = Vec::new();
    for e in extras {
        let (start, end) = (e.start(), e.end());
        // Children are in order, so the one that could hold the comment is
        // the first ending at or after it.
        let k = items.partition_point(|i| i.end() < end);
        let holds = items.get(k).is_some_and(|i| match i.view() {
            View::Node(n) => {
                !n.children.is_empty() && !n.extra && i.start() < start && end <= i.end()
            }
            View::Token(_) => false,
        });
        if holds {
            match into.last_mut() {
                Some((last, list)) if *last == k => list.push(e),
                _ => into.push((k, vec![e])),
            }
        } else {
            here.push(e);
        }
    }
    for (k, list) in into {
        if let Some(children) = items[k].children_mut() {
            place(children, list);
        }
    }
    if here.is_empty() {
        return;
    }
    let old = std::mem::take(items);
    let mut placed = Vec::with_capacity(old.len() + here.len());
    let mut pending = here.into_iter().peekable();
    for item in old {
        let at = item.start();
        while let Some(e) = pending.peek() {
            if e.end() > at {
                break;
            }
            if let Some(e) = pending.next() {
                placed.push(e);
            }
        }
        placed.push(item);
    }
    placed.extend(pending);
    *items = placed;
}

/// `items` by where they start, where a parser left them out of order: a
/// zero-width item (a missing token placed at the previous token's end)
/// after a later one (a statement's hidden end placed past the comments
/// after it), in a file read with errors. Only zero-width items and empty
/// nodes move: tokens with text are always in order.
fn in_order<E: Element>(items: &mut [E]) {
    let key = |i: &E| match i.view() {
        View::Node(n) if n.children.is_empty() => None,
        _ => Some(i.start()),
    };
    let mut last = 0;
    let mut sorted = true;
    for item in items.iter() {
        if let Some(at) = key(item) {
            if at < last {
                sorted = false;
                break;
            }
            last = at;
        }
    }
    if sorted {
        return;
    }
    // An empty node (it emits nothing) sorts with the item before it.
    let mut keys = Vec::with_capacity(items.len());
    let mut prev = 0;
    for item in items.iter() {
        let at = key(item).unwrap_or(prev);
        keys.push(at);
        prev = at;
    }
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| keys[i]);
    permute(items, &order);
}

/// `items` rearranged so that position `n` holds what was at `order[n]`.
fn permute<E>(items: &mut [E], order: &[usize]) {
    let mut done = vec![false; items.len()];
    for start in 0..items.len() {
        if done[start] {
            continue;
        }
        let mut at = start;
        loop {
            done[at] = true;
            let from = order[at];
            if from == start {
                break;
            }
            items.swap(at, from);
            // What was at `from` is now at `at`; `from`'s slot holds the
            // item that still needs a home.
            at = from;
        }
    }
}

/// The tree of `text`: a root node of kind `root` holding `items`, built
/// with `names`; `errors` when the parse met a syntax error no node shows.
pub fn build<E: Element>(
    text: &str,
    names: &Names,
    root: &str,
    items: &[E],
    errors: bool,
) -> Result<Tree, BuildError> {
    let mut b = Builder::new(text, names.grammar());
    if errors {
        b.error_seen();
    }
    b.start_node(
        names.kind(root, true),
        None,
        Flags {
            named: true,
            ..Flags::default()
        },
    )?;
    for item in items {
        emit(&mut b, names, item)?;
    }
    b.finish()
}

/// A tree holding the whole of `text` as one error-marked root, for a parse
/// that could not be built (a parser bug, never a syntax error).
pub fn fallback(text: &str, names: &Names, root: &str) -> Tree {
    let mut b = Builder::new(text, names.grammar());
    let _ = b.start_node(names.kind(root, true), None, Flags::default());
    b.error_seen();
    b.finish()
        .unwrap_or_else(|_| unreachable!("a root over the text always builds"))
}

fn emit<E: Element>(b: &mut Builder, names: &Names, item: &E) -> Result<(), BuildError> {
    let start = item.start();
    if start > b.pos() {
        b.trivia_to(start)?;
    }
    match item.view() {
        View::Token(t) if t.hidden => b.trivia_to(t.end),
        View::Token(t) => {
            let flags = Flags {
                named: t.named,
                missing: t.missing,
                extra: t.extra,
                ..Flags::default()
            };
            b.token(
                names.kind(t.kind, t.named),
                names.field(t.field),
                flags,
                t.end - t.start,
            )
        }
        View::Node(n) if n.kind.is_empty() => {
            for child in n.children {
                emit(b, names, child)?;
            }
            Ok(())
        }
        View::Node(n) => {
            if n.children.is_empty() {
                return Ok(());
            }
            let len = item.end() - start;
            if names.text_leaves
                && len > 0
                && n.children
                    .iter()
                    .all(|i| matches!(i.view(), View::Token(t) if t.hidden))
            {
                let flags = Flags {
                    named: n.named,
                    ..Flags::default()
                };
                return b.token(
                    names.kind(n.kind, n.named),
                    names.field(n.field),
                    flags,
                    len,
                );
            }
            b.start_node(
                names.kind(n.kind, n.named),
                names.field(n.field),
                Flags {
                    named: n.named,
                    extra: n.extra,
                    ..Flags::default()
                },
            )?;
            for child in n.children {
                emit(b, names, child)?;
            }
            b.finish_node()
        }
    }
}

/// [`Element`] for a parser's `Item { Tok(Tok), Node(Node) }`, its `Tok`
/// holding `kind`, `named`, `start`, `end`, `field`, `hidden`, `missing`,
/// `extra` and its `Node` `kind`, `field`, `items`; `named` and `extra` are
/// a node's flags, read from it as `$n`:
///
/// ```ignore
/// colab_syntax::element!(Item, Tok, |n| named: true, extra: false);
/// ```
#[macro_export]
macro_rules! element {
    ($item:ident, $tok:ident, |$n:ident| named: $named:expr, extra: $extra:expr) => {
        impl $crate::emit::Element for $item {
            fn view(&self) -> $crate::emit::View<'_, Self> {
                match self {
                    $item::Tok(t) => $crate::emit::View::Token($crate::emit::TokenView {
                        kind: t.kind,
                        named: t.named,
                        field: t.field,
                        start: t.start,
                        end: t.end,
                        hidden: t.hidden,
                        missing: t.missing,
                        extra: t.extra,
                    }),
                    $item::Node($n) => $crate::emit::View::Node($crate::emit::NodeView {
                        kind: $n.kind,
                        named: $named,
                        field: $n.field,
                        extra: $extra,
                        children: &$n.items,
                    }),
                }
            }

            fn children_mut(&mut self) -> Option<&mut Vec<Self>> {
                match self {
                    $item::Node(n) => Some(&mut n.items),
                    $item::Tok(_) => None,
                }
            }

            fn extra(start: usize, end: usize, kind: &'static str) -> Self {
                $item::Tok($tok {
                    kind,
                    named: true,
                    start,
                    end,
                    field: None,
                    hidden: false,
                    missing: false,
                    extra: true,
                })
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::permute;

    #[test]
    fn permute_puts_each_item_where_the_order_says() {
        let mut items = vec!['a', 'b', 'c', 'd', 'e'];
        permute(&mut items, &[2, 0, 4, 1, 3]);
        assert_eq!(items, vec!['c', 'a', 'e', 'b', 'd']);
    }
}
