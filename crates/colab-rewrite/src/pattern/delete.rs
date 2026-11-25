//! Deleting a match, with what belongs to it: its whole line when it
//! stands alone, its doc comments and attributes ([`Leading`]), or the
//! `,` beside it in a list.

use super::*;

/// Where the item matched at `start..end` really starts: before the doc
/// comments and attributes just above it that belong to it ([`Leading`]),
/// each on its own line(s) with no blank line between it and the next.
pub(crate) fn attached_start(
    tree: &tree_sitter::Tree,
    source: &str,
    start: usize,
    end: usize,
    leading: Leading,
) -> usize {
    let Some(mut node) = tree.root_node().descendant_for_byte_range(start, end) else {
        return start;
    };
    // The widest node over exactly the match: the item, not its first token.
    while let Some(parent) = node.parent() {
        if parent.byte_range() != node.byte_range() {
            break;
        }
        node = parent;
    }
    let mut at = start;
    let mut cursor = node;
    while let Some(prev) = cursor.prev_sibling() {
        let text = &source[prev.byte_range()];
        let attached = leading.kinds.contains(&prev.kind())
            || (prev.is_extra() && leading.doc_comments.iter().any(|p| text.starts_with(p)));
        // Only up to the next item: whitespace and at most one line break
        // between them, and nothing before it on its first line.
        let prev_end = prev.start_byte() + text.trim_end().len();
        let gap = &source[prev_end..at];
        let line_start = source[..prev.start_byte()].rfind('\n').map_or(0, |p| p + 1);
        if !attached
            || !gap.trim().is_empty()
            || gap.matches('\n').count() > 1
            || !source[line_start..prev.start_byte()].trim().is_empty()
        {
            break;
        }
        at = prev.start_byte();
        cursor = prev;
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
pub(crate) fn with_separator(
    tree: &tree_sitter::Tree,
    start: usize,
    end: usize,
) -> Option<(usize, usize)> {
    let mut node = tree.root_node().descendant_for_byte_range(start, end)?;
    // The widest node over exactly the match: an argument, not its inner
    // expression.
    while let Some(parent) = node.parent() {
        if parent.byte_range() != node.byte_range() {
            break;
        }
        node = parent;
    }
    let is_comma = |n: &tree_sitter::Node<'_>| !n.is_named() && n.kind() == ",";
    let next = node.next_sibling().filter(is_comma);
    let prev = node.prev_sibling().filter(is_comma);
    match (next, prev) {
        (Some(next), _) => match next.next_named_sibling() {
            Some(element) => Some((node.start_byte(), element.start_byte())),
            None => match prev {
                Some(prev) => Some((
                    prev.prev_named_sibling()
                        .map_or(prev.start_byte(), |e| e.end_byte()),
                    node.end_byte(),
                )),
                None => Some((node.start_byte(), next.end_byte())),
            },
        },
        (None, Some(prev)) => Some((
            prev.prev_named_sibling()
                .map_or(prev.start_byte(), |e| e.end_byte()),
            node.end_byte(),
        )),
        (None, None) => None,
    }
}
