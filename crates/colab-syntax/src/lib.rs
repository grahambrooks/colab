//! colab's own syntax tree: lossless, immutable, and independent of any
//! parser.
//!
//! Every byte of the source belongs to exactly one *token* (a leaf), so the
//! tokens' texts, in order, are the file. Text a parser does not report —
//! the whitespace tree-sitter skips — is a [`Kind::TRIVIA`] token in the
//! innermost node open at that point. Nodes are the structure over the
//! tokens: a kind, a named flag, an optional field name in their parent,
//! and a byte range that is exactly the span of their children.
//!
//! A tree is built through one [`Builder`] (`start_node` / `token` /
//! `finish_node`), whatever the parser: today [`Tree::from_tree_sitter`]
//! drives it from a tree-sitter parse (feature `tree-sitter`); a parser of
//! colab's own will drive it from its events. Kinds and fields are numbers
//! whose names a [`Grammar`] holds, one per language.
//!
//! Nothing here recurses over the tree: building, iterating and dropping
//! are loops, so a file nested any depth is safe to hold and walk.

mod builder;
pub mod fragment;
#[cfg(feature = "tree-sitter")]
mod from_tree_sitter;
pub mod pattern;

use std::ops::Range;
use std::sync::Arc;

pub use builder::{BuildError, Builder};

/// A parser of colab's own (or an adapter over one) for a language: what
/// takes tree-sitter's place for that language, one at a time. `parse`
/// always gives a tree, the text it could not read in `ERROR` nodes, and
/// marks a parse error it shows no node for ([`Builder::error_seen`]).
pub trait SourceParser: Send + Sync {
    /// The names of the kinds and fields its trees use.
    fn grammar(&self) -> Arc<Grammar>;
    /// `text` as a lossless tree.
    fn parse(&self, text: &str) -> Tree;
}

/// A node kind: an index into its [`Grammar`]'s kinds, or one of the two
/// kinds every grammar has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Kind(pub u16);

impl Kind {
    /// A region the parser could not read (tree-sitter's `ERROR`).
    pub const ERROR: Kind = Kind(u16::MAX);
    /// Text no parser node covers: whitespace between tokens.
    pub const TRIVIA: Kind = Kind(u16::MAX - 1);
}

/// A field name in a parent (`function`, `arguments`, `name`): an index
/// into its [`Grammar`]'s fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Field(pub u16);

/// The names of one language's kinds and fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grammar {
    name: Box<str>,
    /// Kind `i`'s name and whether it is named, by index.
    kinds: Vec<(Box<str>, bool)>,
    /// Field `i`'s name, by index (tree-sitter's ids start at 1; slot 0 is
    /// unused there).
    fields: Vec<Box<str>>,
    /// Other names for kinds: a name another parser gave the same
    /// construct (tree-sitter's `call_expression` for rust-analyzer's
    /// `call_expr` and `method_call_expr`), which a rule may still spell.
    aliases: Vec<(Box<str>, Vec<Kind>)>,
}

impl Grammar {
    /// A grammar from its kinds (name, named) and field names, each indexed
    /// by its id.
    pub fn new(
        name: impl Into<Box<str>>,
        kinds: Vec<(Box<str>, bool)>,
        fields: Vec<Box<str>>,
    ) -> Self {
        Self {
            name: name.into(),
            kinds,
            fields,
            aliases: Vec::new(),
        }
    }

    /// The grammar with `name` also standing for each of `kinds` (by
    /// their own names; one it does not have is skipped).
    #[must_use]
    pub fn with_alias(mut self, name: &str, kinds: &[&str]) -> Self {
        let ids: Vec<Kind> = kinds.iter().flat_map(|k| self.kinds_named(k)).collect();
        if !ids.is_empty() {
            self.aliases.push((name.into(), ids));
        }
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The name of `kind`: `ERROR` and `trivia` for the shared kinds, `?`
    /// for an id the grammar does not have.
    pub fn kind_name(&self, kind: Kind) -> &str {
        match kind {
            Kind::ERROR => "ERROR",
            Kind::TRIVIA => "trivia",
            Kind(i) => self.kinds.get(usize::from(i)).map_or("?", |(n, _)| n),
        }
    }

    /// Whether `kind` is a named kind (an `identifier`, not a `(`).
    /// `ERROR` is named; trivia is not.
    pub fn kind_is_named(&self, kind: Kind) -> bool {
        match kind {
            Kind::ERROR => true,
            Kind::TRIVIA => false,
            Kind(i) => self.kinds.get(usize::from(i)).is_some_and(|(_, n)| *n),
        }
    }

    pub fn field_name(&self, field: Field) -> &str {
        self.fields.get(usize::from(field.0)).map_or("?", |n| n)
    }

    /// The names of the named kinds, sorted, each once: what a "did you
    /// mean" suggests from.
    pub fn named_kind_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .kinds
            .iter()
            .filter(|(_, named)| *named)
            .map(|(n, _)| &**n)
            .chain(self.aliases.iter().map(|(n, _)| &**n))
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// The field names, each once.
    pub fn field_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .fields
            .iter()
            .map(|n| &**n)
            .filter(|n| !n.is_empty())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Every kind called `name`: an aliased kind has more than one id, and
    /// an alias ([`Grammar::with_alias`]) stands for the kinds it names.
    pub fn kinds_named(&self, name: &str) -> Vec<Kind> {
        let mut out: Vec<Kind> = self
            .kinds
            .iter()
            .enumerate()
            .filter(|(_, (n, _))| &**n == name)
            .filter_map(|(i, _)| u16::try_from(i).ok().map(Kind))
            .collect();
        for (alias, kinds) in &self.aliases {
            if &**alias == name {
                out.extend(kinds);
            }
        }
        out
    }

    /// The field called `name`, if the grammar has one.
    pub fn field(&self, name: &str) -> Option<Field> {
        let i = self.fields.iter().position(|f| &**f == name)?;
        u16::try_from(i).ok().map(Field)
    }
}

/// Marks on a node beyond its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    /// A leaf: it holds text, not children.
    pub token: bool,
    /// A named kind (`identifier`), as opposed to an anonymous one (`(`).
    pub named: bool,
    /// Inserted by the parser's recovery: zero-width, standing for text
    /// that is not there (tree-sitter's `MISSING`).
    pub missing: bool,
    /// Allowed anywhere, outside the grammar's structure: a comment.
    pub extra: bool,
}

const NONE: usize = usize::MAX;

#[derive(Debug, Clone)]
struct NodeData {
    kind: Kind,
    field: Option<Field>,
    flags: Flags,
    start: usize,
    end: usize,
    parent: usize,
    first_child: usize,
    last_child: usize,
    next_sibling: usize,
    prev_sibling: usize,
    /// One past this node's last descendant in document order, so a
    /// subtree is a slice of the node list.
    after: usize,
}

/// A parsed file: its text, its grammar's names, and its nodes.
#[derive(Debug, Clone)]
pub struct Tree {
    text: String,
    grammar: Arc<Grammar>,
    /// In document order (a node before its descendants); index 0 is the
    /// root, which spans the whole text.
    nodes: Vec<NodeData>,
    /// The parser reported an error no node shows.
    errors: bool,
}

impl Tree {
    pub fn root(&self) -> Node<'_> {
        Node { tree: self, id: 0 }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn grammar(&self) -> &Grammar {
        &self.grammar
    }

    /// How many nodes, tokens included.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Whether the parse found a syntax error: any node is an `ERROR` or
    /// missing, or the parser said so ([`Builder::error_seen`]) about one
    /// it reported no node for (tree-sitter's missing hidden tokens, such
    /// as the newline Go wants at the end of a file).
    pub fn has_error(&self) -> bool {
        self.errors
            || self
                .nodes
                .iter()
                .any(|n| n.kind == Kind::ERROR || n.flags.missing)
    }

    /// Every node, in document order.
    pub fn nodes(&self) -> impl Iterator<Item = Node<'_>> {
        (0..self.nodes.len()).map(move |id| Node { tree: self, id })
    }

    /// The tokens, in order: their texts concatenated are the file.
    pub fn tokens(&self) -> impl Iterator<Item = Node<'_>> {
        self.nodes().filter(Node::is_token)
    }
}

/// A node of a [`Tree`]: a cheap handle, as tree-sitter's is.
#[derive(Clone, Copy)]
pub struct Node<'t> {
    tree: &'t Tree,
    id: usize,
}

impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.tree, other.tree) && self.id == other.id
    }
}

impl Eq for Node<'_> {}

impl std::fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {:?}", self.kind_name(), self.range())
    }
}

impl<'t> Node<'t> {
    fn data(&self) -> &'t NodeData {
        &self.tree.nodes[self.id]
    }

    fn at(&self, id: usize) -> Option<Node<'t>> {
        (id != NONE).then_some(Node {
            tree: self.tree,
            id,
        })
    }

    pub fn tree(&self) -> &'t Tree {
        self.tree
    }

    /// The node's position in document order, unique within its tree.
    pub fn id(&self) -> usize {
        self.id
    }

    pub fn kind(&self) -> Kind {
        self.data().kind
    }

    pub fn kind_name(&self) -> &'t str {
        self.tree.grammar.kind_name(self.kind())
    }

    pub fn flags(&self) -> Flags {
        self.data().flags
    }

    pub fn is_token(&self) -> bool {
        self.data().flags.token
    }

    pub fn is_named(&self) -> bool {
        self.data().flags.named
    }

    pub fn is_error(&self) -> bool {
        self.kind() == Kind::ERROR
    }

    pub fn is_missing(&self) -> bool {
        self.data().flags.missing
    }

    pub fn is_extra(&self) -> bool {
        self.data().flags.extra
    }

    pub fn is_trivia(&self) -> bool {
        self.kind() == Kind::TRIVIA
    }

    /// The field this node is in its parent, if any.
    pub fn field(&self) -> Option<Field> {
        self.data().field
    }

    pub fn field_name(&self) -> Option<&'t str> {
        self.field().map(|f| self.tree.grammar.field_name(f))
    }

    pub fn range(&self) -> Range<usize> {
        self.data().start..self.data().end
    }

    pub fn start(&self) -> usize {
        self.data().start
    }

    pub fn end(&self) -> usize {
        self.data().end
    }

    pub fn text(&self) -> &'t str {
        &self.tree.text[self.range()]
    }

    pub fn parent(&self) -> Option<Node<'t>> {
        self.at(self.data().parent)
    }

    pub fn first_child(&self) -> Option<Node<'t>> {
        self.at(self.data().first_child)
    }

    pub fn last_child(&self) -> Option<Node<'t>> {
        self.at(self.data().last_child)
    }

    pub fn next_sibling(&self) -> Option<Node<'t>> {
        self.at(self.data().next_sibling)
    }

    pub fn prev_sibling(&self) -> Option<Node<'t>> {
        self.at(self.data().prev_sibling)
    }

    /// Every child, tokens and trivia included.
    pub fn children(&self) -> impl Iterator<Item = Node<'t>> + use<'t> {
        std::iter::successors(self.first_child(), Node::next_sibling)
    }

    /// The named children (no trivia, no anonymous tokens).
    pub fn named_children(&self) -> impl Iterator<Item = Node<'t>> + use<'t> {
        self.children().filter(Node::is_named)
    }

    /// The children in field `name`.
    pub fn children_by_field(&self, name: &str) -> impl Iterator<Item = Node<'t>> + use<'t> {
        let field = self.tree.grammar.field(name);
        self.children()
            .filter(move |c| field.is_some() && c.field() == field)
    }

    /// The first child in field `name`.
    pub fn child_by_field(&self, name: &str) -> Option<Node<'t>> {
        self.children_by_field(name).next()
    }

    /// This node and everything under it, in document order.
    pub fn descendants(&self) -> impl Iterator<Item = Node<'t>> + use<'t> {
        // Document order puts a node's descendants right after it, up to
        // the first node that starts past its subtree.
        let tree = self.tree;
        let end = self.subtree_end();
        (self.id..end).map(move |id| Node { tree, id })
    }

    /// One past the last node of this subtree in document order.
    fn subtree_end(&self) -> usize {
        self.data().after
    }

    /// The smallest node whose range holds `range`, preferring a named one
    /// when a named node and its only token share it.
    pub fn covering(&self, range: Range<usize>) -> Option<Node<'t>> {
        if range.start < self.start() || range.end > self.end() {
            return None;
        }
        let mut node = *self;
        'down: loop {
            for child in node.children() {
                if child.start() <= range.start
                    && range.end <= child.end()
                    && (child.start() < child.end() || range.is_empty())
                {
                    if child.is_token() && !child.is_named() && node.range() == child.range() {
                        return Some(node);
                    }
                    node = child;
                    continue 'down;
                }
            }
            return Some(node);
        }
    }
}

#[cfg(all(test, feature = "tree-sitter"))]
mod pattern_tests;
#[cfg(test)]
mod tests;
