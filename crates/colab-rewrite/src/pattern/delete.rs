//! Deleting a match, with what belongs to it: its whole line when it
//! stands alone, its doc comments and attributes ([`Leading`]), or the
//! `,` beside it in a list.

use super::*;

impl StructuralPattern {
    /// Remove every match. One that stands alone on its line(s) — apart
    /// from indentation and a trailing `;` or `,` — goes with its whole
    /// line. One inside a line goes with the `,` beside it, so a list
    /// element leaves a well-formed list: the following comma and the
    /// space after it (`f(a, b, c)` → `f(a, c)`), or for the last element
    /// the preceding one (`f(a, b)` → `f(a)`). A match with no comma beside
    /// it (`() => f(x)`, `return f(x)`) is left in place: removing it would
    /// leave the code around it incomplete. Rewrite those with `replace`.
    pub fn delete(&self, source: &str) -> String {
        self.delete_in(source, None)
    }

    /// [`delete`](Self::delete), judged in `frame` when `source` is a
    /// macro's contents.
    pub(super) fn delete_in(&self, source: &str, frame: Option<&Frame<'_>>) -> String {
        let mut tree = None;
        let mut parsed = || {
            tree.get_or_insert_with(|| crate::parse_cache::parse(&self.lang.grammar, source))
                .clone()
        };
        let leading = self.lang.leading;
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for found in self.find_file(source, frame) {
            // An item's doc comments and attributes go with it.
            let start = if leading.kinds.is_empty() && leading.doc_comments.is_empty() {
                found.start
            } else {
                parsed().map_or(found.start, |t| {
                    attached_start(&t, source, found.start, found.end, leading)
                })
            };
            let (line_start, line_end) = line_span(source, start, found.end);
            let before = &source[line_start..start];
            let after = &source[found.end..line_end];
            let alone = before.trim().is_empty()
                && after
                    .trim()
                    .trim_start_matches([';', ','])
                    .trim()
                    .is_empty();
            if alone {
                spans.push((
                    line_start,
                    without_double_blank(source, line_start, line_end),
                ));
                continue;
            }
            if let Some(span) = parsed()
                .as_ref()
                .and_then(|t| with_separator(t, found.start, found.end))
            {
                spans.push(span);
            }
        }
        // Outermost first: a span inside one already taken goes with it.
        spans.sort_by_key(|&(start, end)| (start, std::cmp::Reverse(end)));
        let mut kept: Vec<(usize, usize)> = Vec::new();
        for span in spans {
            if kept.last().is_some_and(|last| span.0 < last.1) {
                continue;
            }
            kept.push(span);
        }
        let mut out = source.to_string();
        for &(start, end) in kept.iter().rev() {
            out.replace_range(start..end, "");
        }
        // Then inside macro calls, where the wrapper's tree says which
        // comma goes with an argument, judged where the text now is.
        let next =
            frame.map(|f| f.after(kept.iter().map(|&(s, e)| (s, e, 0)).collect(), out.len()));
        self.edit_macros(&out, next.as_ref(), |wrapped, inner| {
            self.delete_in(wrapped, Some(inner))
        })
    }

    /// `true` when `source` parses without a syntax error.
    pub(super) fn parses_cleanly(&self, source: &str) -> bool {
        parse_clean(&self.lang.grammar, source).is_ok()
    }
}

/// Where the item matched at `start..end` really starts: before the doc
/// comments and attributes just above it that belong to it ([`Leading`]),
/// each on its own line(s) with no blank line between it and the next.
pub(super) fn attached_start(
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
pub(super) fn without_double_blank(source: &str, line_start: usize, line_end: usize) -> usize {
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
pub(super) fn with_separator(
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
