//! Deleting a match, with what belongs to it: its whole line when it
//! stands alone, its doc comments and attributes ([`Leading`]), or the
//! `,` beside it in a list. Judged on colab's own tree, whichever parser
//! made it.

use super::*;
use colab_syntax::{Node, Tree};

/// The widest node over exactly `start..end`: the item, not its first
/// token; an argument, not its inner expression.
fn widest(tree: &Tree, start: usize, end: usize) -> Option<Node<'_>> {
    let mut node = tree.root().covering(start..end)?;
    while let Some(parent) = node.parent() {
        if parent.range() != node.range() {
            break;
        }
        node = parent;
    }
    Some(node)
}

/// `node`'s siblings before (or after) it, trivia passed over.
fn prev(node: Node<'_>) -> Option<Node<'_>> {
    std::iter::successors(node.prev_sibling(), Node::prev_sibling).find(|n| !n.is_trivia())
}

fn next(node: Node<'_>) -> Option<Node<'_>> {
    std::iter::successors(node.next_sibling(), Node::next_sibling).find(|n| !n.is_trivia())
}

fn prev_named(node: Node<'_>) -> Option<Node<'_>> {
    std::iter::successors(prev(node), |n| prev(*n)).find(Node::is_named)
}

fn next_named(node: Node<'_>) -> Option<Node<'_>> {
    std::iter::successors(next(node), |n| next(*n)).find(Node::is_named)
}

/// Where the item spanning `start..end` begins as code: after the doc
/// comments, comments and [`Leading`] kinds a tree holds inside the item
/// (rust-analyzer's), which [`attached_start`] then judges one by one; at
/// the item's start where they are its siblings (tree-sitter's).
pub(crate) fn own_start(tree: &Tree, start: usize, end: usize, leading: Leading) -> usize {
    let Some(node) = widest(tree, start, end) else {
        return start;
    };
    node.children()
        .filter(|c| !c.is_trivia())
        .find(|c| !(c.is_extra() || leading.kinds.contains(&c.kind_name())))
        .map_or(start, |c| c.start().max(start))
}

/// Where the item matched at `start..end` really starts: before the doc
/// comments and attributes just above it that belong to it ([`Leading`]),
/// each on its own line(s) with no blank line between it and the next;
/// those the item holds itself (before `start`) are judged first.
pub(crate) fn attached_start(
    tree: &Tree,
    source: &str,
    start: usize,
    end: usize,
    leading: Leading,
) -> usize {
    let Some(node) = widest(tree, start, end).or_else(|| tree.root().covering(start..end)) else {
        return start;
    };
    // What the node holds before `start`, nearest first, then its siblings
    // before it.
    let mut before: Vec<Node<'_>> = node
        .children()
        .filter(|c| !c.is_trivia() && c.end() <= start)
        .collect();
    before.reverse();
    let mut cursor = node;
    while let Some(p) = prev(cursor) {
        before.push(p);
        cursor = p;
    }
    let mut at = start;
    for before in before {
        let text = before.text();
        let attached = leading.kinds.contains(&before.kind_name())
            || (before.is_extra() && leading.doc_comments.iter().any(|p| text.starts_with(p)));
        // Only up to the next item: whitespace and at most one line break
        // between them, and nothing before it on its first line.
        let before_end = before.start() + text.trim_end().len();
        let gap = &source[before_end..at];
        let line_start = source[..before.start()].rfind('\n').map_or(0, |p| p + 1);
        if !attached
            || !gap.trim().is_empty()
            || gap.matches('\n').count() > 1
            || !source[line_start..before.start()].trim().is_empty()
        {
            break;
        }
        at = before.start();
    }
    at
}

/// `line_end`, or past the blank line after it when the deleted lines
/// `line_start..line_end` sit between two blank lines (or open the file
/// before one): the deletion then leaves one blank line, not two.
pub(crate) fn without_double_blank(source: &str, line_start: usize, line_end: usize) -> usize {
    let blank_before = line_start == 0 || {
        let prev = source[..line_start - 1].rfind('\n').map_or(0, |p| p + 1);
        source[prev..line_start].trim().is_empty()
    };
    let rest = &source[line_end..];
    let next_end = rest.find('\n').map_or(rest.len(), |p| p + 1);
    let next = &rest[..next_end];
    if blank_before && !next.is_empty() && next.trim().is_empty() && next.ends_with('\n') {
        line_end + next_end
    } else {
        line_end
    }
}

/// The bytes to remove for the match at `start..end` with the `,` beside
/// it: up to the next element when a comma follows and an element after
/// it, else from the end of the previous element when a comma precedes,
/// else the match and a trailing comma alone (`f(a,)`). `None` when no
/// comma is beside it.
pub(crate) fn with_separator(tree: &Tree, start: usize, end: usize) -> Option<(usize, usize)> {
    let node = widest(tree, start, end)?;
    let is_comma = |n: &Node<'_>| !n.is_named() && n.text() == ",";
    let after = next(node).filter(is_comma);
    let before = prev(node).filter(is_comma);
    let from_previous = |comma: Node<'_>| prev_named(comma).map_or(comma.start(), |e| e.end());
    match (after, before) {
        (Some(after), _) => match next_named(after) {
            Some(element) => Some((node.start(), element.start())),
            None => match before {
                Some(before) => Some((from_previous(before), node.end())),
                None => Some((node.start(), after.end())),
            },
        },
        (None, Some(before)) => Some((from_previous(before), node.end())),
        (None, None) => None,
    }
}
