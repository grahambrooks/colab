//! One parse per text, shared by the rules that read it (M10.6).
//!
//! Most of a rule's cost is the parse: on 55 MB of Rust, tree-sitter took
//! 5.0 s of the 6.4-8.1 s a `pattern` rule needed. A script's rules run
//! one after another over a file, each parsing the text it is given; when
//! a rule changes nothing, the next is given the same text and parses it
//! again. This cache hands back the tree instead, keyed by the grammar and
//! the exact text, so the rules still see exactly what they saw before
//! (each one the previous one's output): a rule that changes the text
//! makes the next parse a miss. Nothing about what a rule matches or
//! writes depends on it.
//!
//! The cache is per thread, as the walker gives each file to one thread,
//! and holds the last few texts only.
//!
//! Two more things save parses and walks:
//!
//! - [`reparse_edited`]: when a rule's edits turn a cached text into a
//!   new one, the new text is parsed *incrementally* from the old tree,
//!   reusing every subtree the edits did not touch. [`crate::apply_edits`]
//!   calls it, so the check that a rewrite still parses, and the next
//!   rule, find the new tree here at a fraction of a full parse.
//! - [`kind_index`]: the second time a tree is searched, its nodes are
//!   indexed by kind in one walk, so each further search visits only the
//!   nodes its pattern can start at instead of the whole tree.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use tree_sitter::{InputEdit, Language, Parser, Point, Tree};

use crate::Edit;

/// Texts kept: a file's chain of rules moves through one at a time, and a
/// rule's checks may parse its input and its output.
const KEPT: usize = 4;

struct Entry {
    grammar: Language,
    text: String,
    tree: Tree,
    /// Times the tree has been searched ([`kind_index`]).
    walks: Cell<u32>,
    index: OnceCell<Rc<KindIndex>>,
}

impl Entry {
    fn new(grammar: &Language, text: &str, tree: Tree) -> Self {
        Self {
            grammar: grammar.clone(),
            text: text.to_string(),
            tree,
            walks: Cell::new(0),
            index: OnceCell::new(),
        }
    }
}

/// A tree's nodes by kind: for each kind id, the pre-order positions
/// (tree-sitter's descendant indices, root 0) of its nodes, ascending.
pub struct KindIndex {
    /// Indexed by kind id.
    by_kind: Vec<Vec<u32>>,
}

impl KindIndex {
    fn build(tree: &Tree) -> Self {
        let kinds = tree.language().node_kind_count();
        let mut by_kind: Vec<Vec<u32>> = vec![Vec::new(); kinds];
        let mut cursor = tree.walk();
        let mut index = 0u32;
        loop {
            let kind = usize::from(cursor.node().kind_id());
            if kind >= by_kind.len() {
                by_kind.resize_with(kind + 1, Vec::new);
            }
            by_kind[kind].push(index);
            index += 1;
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return Self { by_kind };
                }
            }
        }
    }

    /// The pre-order positions of every node of these kinds, ascending:
    /// the order a pre-order walk would meet them in.
    pub fn positions(&self, kinds: impl IntoIterator<Item = usize>) -> Vec<u32> {
        let mut out: Vec<u32> = kinds
            .into_iter()
            .filter_map(|k| self.by_kind.get(k))
            .flatten()
            .copied()
            .collect();
        out.sort_unstable();
        out
    }
}

thread_local! {
    static CACHE: RefCell<VecDeque<Entry>> = const { RefCell::new(VecDeque::new()) };
    static PARSER: RefCell<Option<(Language, Parser)>> = const { RefCell::new(None) };
}

/// `source` parsed with `grammar`, from the cache when this thread parsed
/// the same text with the same grammar lately.
pub fn parse(grammar: &Language, source: &str) -> Option<Tree> {
    let hit = CACHE.with_borrow(|cache| {
        cache
            .iter()
            .find(|e| e.text.len() == source.len() && e.grammar == *grammar && e.text == source)
            .map(|e| e.tree.clone())
    });
    if hit.is_some() {
        return hit;
    }
    let tree = PARSER.with_borrow_mut(|slot| {
        if slot.as_ref().is_none_or(|(g, _)| g != grammar) {
            let mut parser = Parser::new();
            parser.set_language(grammar).ok()?;
            *slot = Some((grammar.clone(), parser));
        }
        slot.as_mut()?.1.parse(source, None)
    })?;
    keep(Entry::new(grammar, source, tree.clone()));
    Some(tree)
}

fn keep(entry: Entry) {
    CACHE.with_borrow_mut(|cache| {
        if cache.len() == KEPT {
            cache.pop_front();
        }
        cache.push_back(entry);
    });
}

/// `before` with `edits` applied is `after`: cache `after`'s tree,
/// parsed incrementally from each cached tree of `before`. `edits` are
/// sorted by start and do not overlap, as [`crate::apply_edits`] applies
/// them. Does nothing when `before` is not cached or `after` already is.
pub fn reparse_edited(before: &str, edits: &[Edit], after: &str) {
    let olds: Vec<(Language, Tree)> = CACHE.with_borrow(|cache| {
        let cached = |grammar: &Language, text: &str| {
            cache
                .iter()
                .any(|e| e.grammar == *grammar && e.text.len() == text.len() && e.text == text)
        };
        cache
            .iter()
            .filter(|e| e.text.len() == before.len() && e.text == before)
            .filter(|e| !cached(&e.grammar, after))
            .map(|e| (e.grammar.clone(), e.tree.clone()))
            .collect()
    });
    if olds.is_empty() || edits.is_empty() {
        return;
    }
    let input = input_edits(before, edits);
    for (grammar, mut tree) in olds {
        // Last edit first: the text before an edit is still `before`'s, so
        // its old coordinates hold.
        for e in input.iter().rev() {
            tree.edit(e);
        }
        let new = PARSER.with_borrow_mut(|slot| {
            if slot.as_ref().is_none_or(|(g, _)| *g != grammar) {
                let mut parser = Parser::new();
                parser.set_language(&grammar).ok()?;
                *slot = Some((grammar.clone(), parser));
            }
            slot.as_mut()?.1.parse(after, Some(&tree))
        });
        if let Some(new) = new {
            keep(Entry::new(&grammar, after, new));
        }
    }
}

/// tree-sitter's description of each edit, in `before`'s coordinates
/// (rows and byte columns), found in one pass over `before`.
fn input_edits(before: &str, edits: &[Edit]) -> Vec<InputEdit> {
    let bytes = before.as_bytes();
    let (mut at, mut row, mut line_start) = (0usize, 0usize, 0usize);
    let mut point_at = |offset: usize| {
        for (i, b) in bytes.iter().enumerate().take(offset).skip(at) {
            if *b == b'\n' {
                row += 1;
                line_start = i + 1;
            }
        }
        at = at.max(offset);
        Point::new(row, offset - line_start)
    };
    edits
        .iter()
        .map(|e| {
            let start_position = point_at(e.start);
            let old_end_position = point_at(e.end);
            let new_end_position = match e.text.rfind('\n') {
                Some(last) => Point::new(
                    start_position.row + e.text.matches('\n').count(),
                    e.text.len() - last - 1,
                ),
                None => Point::new(start_position.row, start_position.column + e.text.len()),
            };
            InputEdit {
                start_byte: e.start,
                old_end_byte: e.end,
                new_end_byte: e.start + e.text.len(),
                start_position,
                old_end_position,
                new_end_position,
            }
        })
        .collect()
}

/// The kind index of the tree rooted at `tree_root`, a parse of `source`,
/// from the second search of that text on: the first search walks the
/// tree anyway, so a text searched once costs no index. `None` when the
/// text is not cached with that grammar, or on a first search.
pub fn kind_index(source: &str, tree_root: tree_sitter::Node<'_>) -> Option<Rc<KindIndex>> {
    CACHE.with_borrow(|cache| {
        // A clone of a tree is its own `TSTree` (node ids differ), but its
        // shape is the parse's, so the index fits any tree parsed from the
        // same text by the same grammar.
        let entry = cache.iter().find(|e| {
            e.text.len() == source.len()
                && e.grammar == *tree_root.language()
                && e.tree.root_node().descendant_count() == tree_root.descendant_count()
                && e.text == source
        })?;
        let walks = entry.walks.get() + 1;
        entry.walks.set(walks);
        (walks >= 2).then(|| {
            Rc::clone(
                entry
                    .index
                    .get_or_init(|| Rc::new(KindIndex::build(&entry.tree))),
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_text_is_parsed_once_and_a_changed_one_again() {
        let js: Language = tree_sitter_javascript::LANGUAGE.into();
        let go: Language = tree_sitter_go::LANGUAGE.into();
        let a = parse(&js, "function a() {}\n").unwrap();
        let again = parse(&js, "function a() {}\n").unwrap();
        // The cached tree is the one parsed: same root, same text.
        assert_eq!(a.root_node().to_sexp(), again.root_node().to_sexp());
        let b = parse(&js, "function b() {}\n").unwrap();
        assert_eq!(b.root_node().to_sexp(), a.root_node().to_sexp());
        // A different grammar on the same text is its own parse.
        let as_go = parse(&go, "function a() {}\n").unwrap();
        assert!(as_go.root_node().has_error());
        assert!(
            !parse(&js, "function a() {}\n")
                .unwrap()
                .root_node()
                .has_error()
        );
    }

    #[test]
    fn an_edited_text_is_reparsed_to_the_tree_a_fresh_parse_gives() {
        let go: Language = tree_sitter_go::LANGUAGE.into();
        let before =
            "package p\n\nfunc a() {\n\tx := f().Unwrap()\n\tg(x).Unwrap()\n}\n\nfunc b() {}\n";
        parse(&go, before).unwrap();
        // Several edits, one adding lines, one removing them.
        let first = before.find(".Unwrap()").unwrap();
        let second = before.rfind(".Unwrap()").unwrap();
        let tail = before.find("\n\nfunc b").unwrap();
        let edits = vec![
            Edit::new(first, first + 9, ".Expect(\"x\")"),
            Edit::new(second, second + 9, ".Expect(\n\t\t\"y\",\n\t)"),
            Edit::new(tail, tail + 13, ""),
        ];
        let after = crate::apply_edits(before, edits);
        let cached = parse(&go, &after).unwrap();
        let mut parser = Parser::new();
        parser.set_language(&go).unwrap();
        let fresh = parser.parse(&after, None).unwrap();
        assert_eq!(cached.root_node().to_sexp(), fresh.root_node().to_sexp());
        assert_eq!(
            cached.root_node().end_position(),
            fresh.root_node().end_position()
        );
    }

    #[test]
    fn the_kind_index_lists_a_kinds_nodes_in_pre_order() {
        let go: Language = tree_sitter_go::LANGUAGE.into();
        let text = "package p\nfunc a() { f(g(1)) }\nfunc b() { h() }\n";
        let tree = parse(&go, text).unwrap();
        // The first search walks the tree itself; the index comes after.
        assert!(kind_index(text, tree.root_node()).is_none());
        let index = kind_index(text, tree.root_node()).unwrap();
        let call = go.id_for_node_kind("call_expression", true);
        let mut cursor = tree.walk();
        let found: Vec<String> = index
            .positions([usize::from(call)])
            .into_iter()
            .map(|p| {
                cursor.goto_descendant(p as usize);
                cursor
                    .node()
                    .utf8_text(text.as_bytes())
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(found, ["f(g(1))", "g(1)", "h()"]);
    }
}
