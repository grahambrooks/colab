//! The one way to make a [`Tree`]: open a node, add tokens and child
//! nodes, close it. Positions are implicit: each token takes the next
//! `len` bytes of the text, and a node spans what was added inside it.

use std::sync::Arc;

use crate::{Field, Flags, Grammar, Kind, NONE, NodeData, Tree};

/// Why a [`Builder`] could not make a tree: a parser (or adapter) bug,
/// never the source's fault — any text, even a malformed one, has a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError(pub String);

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "syntax tree: {}", self.0)
    }
}

impl std::error::Error for BuildError {}

/// Builds a [`Tree`] over `text` from a parser's events.
///
/// The first node opened is the root, and it starts at byte 0; tokens and
/// trivia fill the text from there, in order. [`finish`](Self::finish)
/// puts any text left after the last token in the root as trivia, so the
/// tree always covers the whole file.
pub struct Builder {
    text: String,
    grammar: Arc<Grammar>,
    nodes: Vec<NodeData>,
    open: Vec<usize>,
    pos: usize,
    errors: bool,
}

impl Builder {
    pub fn new(text: impl Into<String>, grammar: Arc<Grammar>) -> Self {
        Self {
            text: text.into(),
            grammar,
            nodes: Vec::new(),
            open: Vec::new(),
            pos: 0,
            errors: false,
        }
    }

    /// Record that the parse has a syntax error the tree shows no node
    /// for, so [`Tree::has_error`] still reports it.
    pub fn error_seen(&mut self) {
        self.errors = true;
    }

    /// The byte offset the next token starts at.
    pub fn pos(&self) -> usize {
        self.pos
    }

    fn push(&mut self, kind: Kind, field: Option<Field>, flags: Flags, end: usize) -> usize {
        let id = self.nodes.len();
        let parent = self.open.last().copied().unwrap_or(NONE);
        let prev = if parent == NONE {
            NONE
        } else {
            self.nodes[parent].last_child
        };
        self.nodes.push(NodeData {
            kind,
            field,
            flags,
            start: self.pos,
            end,
            parent,
            first_child: NONE,
            last_child: NONE,
            next_sibling: NONE,
            prev_sibling: prev,
            after: id + 1,
        });
        if parent != NONE {
            if prev == NONE {
                self.nodes[parent].first_child = id;
            } else {
                self.nodes[prev].next_sibling = id;
            }
            self.nodes[parent].last_child = id;
        }
        id
    }

    /// Open a node of `kind` at the current position.
    pub fn start_node(
        &mut self,
        kind: Kind,
        field: Option<Field>,
        flags: Flags,
    ) -> Result<(), BuildError> {
        if self.open.is_empty() && !self.nodes.is_empty() {
            return Err(BuildError("a second root node".into()));
        }
        let flags = Flags {
            token: false,
            ..flags
        };
        let id = self.push(kind, field, flags, self.pos);
        self.open.push(id);
        Ok(())
    }

    /// Close the innermost open node; it ends where its last token did.
    pub fn finish_node(&mut self) -> Result<(), BuildError> {
        if self.open.len() == 1 {
            // The root stays open until `finish`, which fills it to the end.
            return Err(BuildError(
                "the root is closed by `finish`, not `finish_node`".into(),
            ));
        }
        let id = self
            .open
            .pop()
            .ok_or_else(|| BuildError("`finish_node` with no open node".into()))?;
        self.close(id);
        Ok(())
    }

    fn close(&mut self, id: usize) {
        self.nodes[id].end = self.pos;
        self.nodes[id].after = self.nodes.len();
    }

    /// A token of `kind` over the next `len` bytes (zero for a missing one).
    pub fn token(
        &mut self,
        kind: Kind,
        field: Option<Field>,
        flags: Flags,
        len: usize,
    ) -> Result<(), BuildError> {
        if self.open.is_empty() {
            return Err(BuildError("a token outside the root".into()));
        }
        let end = self.pos + len;
        if end > self.text.len() || !self.text.is_char_boundary(end) {
            return Err(BuildError(format!(
                "a token at {}..{end} is not inside the text ({} bytes) on character boundaries",
                self.pos,
                self.text.len()
            )));
        }
        let flags = Flags {
            token: true,
            ..flags
        };
        self.push(kind, field, flags, end);
        self.pos = end;
        Ok(())
    }

    /// Trivia over the text from the current position to `offset`, in the
    /// innermost open node; nothing when `offset` is not past it.
    pub fn trivia_to(&mut self, offset: usize) -> Result<(), BuildError> {
        if offset > self.pos {
            self.token(Kind::TRIVIA, None, Flags::default(), offset - self.pos)?;
        }
        Ok(())
    }

    /// The tree: the text left after the last token is trivia in the root,
    /// which must be the only node still open.
    pub fn finish(mut self) -> Result<Tree, BuildError> {
        match self.open.len() {
            0 => return Err(BuildError("no root node".into())),
            1 => {}
            n => return Err(BuildError(format!("{} nodes left open", n - 1))),
        }
        let end = self.text.len();
        self.trivia_to(end)?;
        let root = self.open.pop().unwrap_or(0);
        self.close(root);
        Ok(Tree {
            text: self.text,
            grammar: self.grammar,
            nodes: self.nodes,
            errors: self.errors,
        })
    }
}
