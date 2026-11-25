//! A [`Tree`] from a tree-sitter parse, while colab's languages still
//! parse with tree-sitter: same kinds (by tree-sitter's ids), fields and
//! ranges, plus trivia tokens for the whitespace tree-sitter leaves out,
//! so the tree is lossless.

use std::sync::Arc;

use crate::{BuildError, Builder, Field, Flags, Grammar, Kind, Tree};

impl Grammar {
    /// The kind and field names of a tree-sitter language, by its own ids.
    pub fn from_tree_sitter(name: &str, language: &tree_sitter::Language) -> Self {
        let kinds = (0..language.node_kind_count())
            .filter_map(|i| u16::try_from(i).ok())
            .map(|id| {
                let name = language.node_kind_for_id(id).unwrap_or("?");
                (Box::from(name), language.node_kind_is_named(id))
            })
            .collect();
        // Field ids start at 1; slot 0 stays empty.
        let fields = (0..=language.field_count())
            .filter_map(|i| u16::try_from(i).ok())
            .map(|id| Box::from(language.field_name_for_id(id).unwrap_or("")))
            .collect();
        Grammar::new(name, kinds, fields)
    }
}

fn flags(node: tree_sitter::Node<'_>) -> Flags {
    Flags {
        token: false,
        named: node.is_named(),
        missing: node.is_missing(),
        extra: node.is_extra(),
    }
}

impl Tree {
    /// `tree` (a parse of `text`) as a colab tree. A node without children
    /// is a token; a gap between tree-sitter's nodes is trivia in the
    /// innermost node around it, so every node keeps tree-sitter's range
    /// except the root, which spans the whole text.
    pub fn from_tree_sitter(
        tree: &tree_sitter::Tree,
        text: &str,
        grammar: Arc<Grammar>,
    ) -> Result<Tree, BuildError> {
        let mut b = Builder::new(text, grammar);
        let mut cursor = tree.walk();
        let root = cursor.node();
        // The cursor never visits a missing hidden token (Go's final
        // newline), so take the parser's word for an error.
        if root.has_error() {
            b.error_seen();
        }
        b.start_node(Kind(root.kind_id()), None, flags(root))?;
        if !cursor.goto_first_child() {
            return b.finish();
        }
        loop {
            let node = cursor.node();
            let field = cursor.field_id().map(|f| Field(f.get()));
            if node.start_byte() < b.pos() {
                return Err(BuildError(format!(
                    "tree-sitter's `{}` at {} starts before the previous node ends ({})",
                    node.kind(),
                    node.start_byte(),
                    b.pos()
                )));
            }
            b.trivia_to(node.start_byte())?;
            if node.child_count() > 0 {
                b.start_node(Kind(node.kind_id()), field, flags(node))?;
                cursor.goto_first_child();
                continue;
            }
            b.token(
                Kind(node.kind_id()),
                field,
                flags(node),
                node.end_byte() - node.start_byte(),
            )?;
            // Up to the next sibling, closing each node left on the way.
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() || cursor.depth() == 0 {
                    return b.finish();
                }
                b.trivia_to(cursor.node().end_byte())?;
                b.finish_node()?;
            }
        }
    }
}
