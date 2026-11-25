//! Tree-sitter rewriting primitives shared by every language backend.
//!
//! Twelve backends do the same four things to a syntax tree: walk it,
//! collect byte-range edits, apply those edits without invalidating
//! offsets, and delete whole lines. Before this crate existed each one
//! carried its own copy — the `apply_edits` helper appeared in ten
//! crates, the whole-line delete span calculation in twelve, and the
//! identifier-rename loop was byte-for-byte identical in all twelve.
//!
//! That was connascence of algorithm across twelve separate crates: a
//! bug in the shared logic had to be found and fixed twelve times, and
//! the "apply edits in reverse byte order" rule was enforced only by
//! documentation. Here it is enforced by there being one implementation.
//! The same holds one level up: [`language`] is the whole of
//! `<lang>::symbol`, `<lang>::call` and `<lang>::pattern`, which each
//! source backend configures with a [`SourceLanguage`].
//!
//! This crate depends on `colab-core`, never the reverse. Keeping the
//! parser dependency out of `colab-core` is what stops `colab-dsl` and
//! `colab-mcp`, neither of which parses anything, from inheriting a
//! grammar engine.

pub mod calls;
pub mod declarations;
pub use declarations::declarations;
mod derive;
pub mod generalize;
pub mod language;
pub use language::{CallShape, SourceLanguage};
pub mod parse_cache;
pub mod pattern;
pub mod scope;
pub mod syntax;

use tree_sitter::{Node, Tree, TreeCursor};

/// A pending replacement of one byte range with new text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Edit {
    pub fn new(start: usize, end: usize, text: impl Into<String>) -> Self {
        Self {
            start,
            end,
            text: text.into(),
        }
    }

    /// An edit replacing a node's span.
    pub fn replacing(node: Node<'_>, text: impl Into<String>) -> Self {
        Self::new(node.start_byte(), node.end_byte(), text)
    }
}

/// Apply `edits` to `source`, returning the rewritten string.
///
/// **Edits are applied in reverse byte order**, which is what keeps
/// earlier offsets valid as later ones are replaced. Applying them
/// forwards would shift every subsequent range by the length delta of
/// the edit before it. This is the single most replicated invariant in
/// the backends and the reason this function exists exactly once.
///
/// An empty edit list returns `source` unchanged — callers rely on
/// string equality with the input to signal "nothing to do", so this
/// must not rebuild an identical-but-new string via a different path.
/// Overlapping edits are applied outermost-last and are the caller's
/// problem; no backend produces them, because each collects at most one
/// edit per node.
pub fn apply_edits(source: &str, mut edits: Vec<Edit>) -> String {
    if edits.is_empty() {
        return source.to_string();
    }
    edits.sort_by_key(|e| e.start);
    if edits.windows(2).any(|w| w[1].start < w[0].end) {
        // Overlapping edits: applied last first, as they always were; the
        // tree cannot be edited to match, so the next parse is a full one.
        let mut out = source.to_string();
        for edit in edits.iter().rev() {
            out.replace_range(edit.start..edit.end, &edit.text);
        }
        return out;
    }
    // One pass forward, rather than a `replace_range` (and a shift of the
    // rest of the text) per edit.
    let grown: usize = edits.iter().map(|e| e.text.len()).sum();
    let mut out = String::with_capacity(source.len() + grown);
    let mut at = 0;
    for edit in &edits {
        out.push_str(&source[at..edit.start]);
        out.push_str(&edit.text);
        at = edit.end;
    }
    out.push_str(&source[at..]);
    // The next parse of `out` — the check that it still parses, the next
    // rule — reuses the tree of `source` (`parse_cache`).
    parse_cache::reparse_edited(source, &edits, &out);
    out
}

/// The byte range of the whole line(s) containing `start..end`,
/// including the trailing newline when there is one.
///
/// Used by every `delete` operation: removing an import leaves a blank
/// line unless the line terminator goes with it.
pub fn line_span(source: &str, start: usize, end: usize) -> (usize, usize) {
    let line_start = source[..start].rfind('\n').map_or(0, |p| p + 1);
    let line_end = source[end..]
        .find('\n')
        .map_or(source.len(), |p| end + p + 1);
    (line_start, line_end)
}

/// Delete the whole lines spanned by each `(start, end)` byte range.
///
/// Ranges are deduplicated, so two matches on the same line delete it
/// once rather than eating the following line.
pub fn delete_lines(source: &str, spans: Vec<(usize, usize)>) -> String {
    if spans.is_empty() {
        return source.to_string();
    }
    let mut spans: Vec<(usize, usize)> = spans
        .into_iter()
        .map(|(start, end)| line_span(source, start, end))
        .collect();
    spans.sort_unstable();
    spans.dedup();
    let mut out = source.to_string();
    for (start, end) in spans.iter().rev() {
        out.replace_range(*start..*end, "");
    }
    out
}

/// Visit every node in `tree`, depth-first, parents before children.
///
/// Replaces the hand-rolled `cursor.goto_first_child()` recursion that
/// appeared 42 times across the backends.
pub fn visit_all<F>(tree: &Tree, mut visit: F)
where
    F: FnMut(Node<'_>),
{
    let mut cursor = tree.walk();
    descend(&mut cursor, &mut visit);
}

fn descend<F>(cursor: &mut TreeCursor<'_>, visit: &mut F)
where
    F: FnMut(Node<'_>),
{
    visit(cursor.node());
    if cursor.goto_first_child() {
        loop {
            descend(cursor, visit);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// Rename every node whose kind is in `kinds` and whose text equals
/// `from`, replacing it with `to`.
///
/// This is the whole of `<lang>::symbol` for all twelve backends; they
/// differ only in which node kinds count as an identifier. Matching is
/// on exact node text, never a substring, so `tokio` never matches
/// `my_tokio`. There is no scope analysis: a local sharing a name with a
/// type is renamed too, which is why the DSL offers `in "<glob>"`.
pub fn rename_nodes_by_text(
    tree: &Tree,
    source: &str,
    kinds: &[&str],
    from: &str,
    to: &str,
) -> String {
    let mut edits = Vec::new();
    visit_all(tree, |node| {
        if kinds.contains(&node.kind())
            && let Ok(text) = node.utf8_text(source.as_bytes())
            && text == from
        {
            edits.push(Edit::replacing(node, to));
        }
    });
    apply_edits(source, edits)
}

/// The byte ranges of every node whose kind is in `kinds`: a backend's
/// comments and string contents, for
/// `LanguageBackend::prose_ranges`. Ranges may nest.
pub fn ranges_of_kinds(tree: &Tree, kinds: &[&str]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    visit_all(tree, |node| {
        if kinds.contains(&node.kind()) {
            out.push(node.byte_range());
        }
    });
    out
}

/// The conflict a [`rename_nodes_by_text`] from `from` to `to` would
/// cause in `source`: `to` already occurs there as an identifier, so the
/// rename would merge two names into one. `None` when it does not, or
/// when `from` does not occur (the rename would not fire).
pub fn rename_conflict(
    tree: &Tree,
    source: &str,
    kinds: &[&str],
    from: &str,
    to: &str,
) -> Option<String> {
    let (mut has_from, mut lines) = (false, Vec::new());
    visit_all(tree, |node| {
        if !kinds.contains(&node.kind()) {
            return;
        }
        match node.utf8_text(source.as_bytes()) {
            Ok(text) if text == from => has_from = true,
            Ok(text) if text == to => lines.push(node.start_position().row + 1),
            _ => {}
        }
    });
    if !has_from || lines.is_empty() {
        return None;
    }
    lines.dedup();
    let shown: Vec<String> = lines.iter().take(5).map(usize::to_string).collect();
    let more = if lines.len() > 5 { ", …" } else { "" };
    Some(format!(
        "`{to}` already occurs in this file (line {}{more}), so renaming `{from}` to it merges two names",
        shown.join(", ")
    ))
}

/// The text of the `field` child of every node whose kind is in `kinds`,
/// with any of `trim` stripped from both ends.
///
/// The read-only twin of the matchers: where a rule asks "is there an
/// import `a/b`?", this answers "which imports are there?" — the values
/// near-miss reporting compares a dead rule's target against. Pass
/// `field: None` to take the node's own text.
pub fn field_texts(
    tree: &Tree,
    source: &str,
    kinds: &[&str],
    field: Option<&str>,
    trim: &[char],
) -> Vec<String> {
    let mut out = Vec::new();
    visit_all(tree, |node| {
        if !kinds.contains(&node.kind()) {
            return;
        }
        let target = match field {
            Some(name) => node.child_by_field_name(name),
            None => Some(node),
        };
        if let Some(target) = target
            && let Ok(text) = target.utf8_text(source.as_bytes())
        {
            out.push(text.trim_matches(trim).to_string());
        }
    });
    out
}

/// Collect the source text of each named child of `node`.
///
/// Used by every `replace_call` implementation to build the `$1`/`$args`
/// substitution list from an argument-list node.
pub fn named_child_texts<'a>(node: Node<'a>, source: &'a str) -> Vec<String> {
    node.named_children(&mut node.walk())
        .filter_map(|child| child.utf8_text(source.as_bytes()).ok())
        .map(str::to_string)
        .collect()
}

/// The first named child of `node` whose kind is in `kinds`, with its text.
pub fn first_child_of_kind<'a>(
    node: Node<'a>,
    source: &'a str,
    kinds: &[&str],
) -> Option<(Node<'a>, &'a str)> {
    node.named_children(&mut node.walk())
        .filter(|child| kinds.contains(&child.kind()))
        .find_map(|child| child.utf8_text(source.as_bytes()).ok().map(|t| (child, t)))
}

/// The **last** named child of `node` whose kind is in `kinds`, with its
/// text.
///
/// C# needs this: `using Alias = Foo.Bar;` exposes both the alias and the
/// imported name as children, and the target is the imported name.
pub fn last_child_of_kind<'a>(
    node: Node<'a>,
    source: &'a str,
    kinds: &[&str],
) -> Option<(Node<'a>, &'a str)> {
    let mut found = None;
    for child in node.named_children(&mut node.walk()) {
        if kinds.contains(&child.kind())
            && let Ok(text) = child.utf8_text(source.as_bytes())
        {
            found = Some((child, text));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_edits_return_the_input() {
        assert_eq!(apply_edits("abc", Vec::new()), "abc");
    }

    #[test]
    fn edits_apply_in_reverse_byte_order() {
        // Two edits with different length deltas. Applied forwards, the
        // second range would be shifted and corrupt the output — this is
        // the invariant the whole crate exists to hold in one place.
        let source = "aaa bbb ccc";
        let edits = vec![Edit::new(0, 3, "LONGER"), Edit::new(8, 11, "X")];
        assert_eq!(apply_edits(source, edits), "LONGER bbb X");
    }

    #[test]
    fn edits_are_order_independent_on_input() {
        let source = "aaa bbb ccc";
        let forward = apply_edits(source, vec![Edit::new(0, 3, "1"), Edit::new(8, 11, "2")]);
        let reverse = apply_edits(source, vec![Edit::new(8, 11, "2"), Edit::new(0, 3, "1")]);
        assert_eq!(forward, reverse);
        assert_eq!(forward, "1 bbb 2");
    }

    #[test]
    fn line_span_covers_the_trailing_newline() {
        let source = "one\ntwo\nthree\n";
        let (start, end) = line_span(source, 4, 7);
        assert_eq!(&source[start..end], "two\n");
    }

    #[test]
    fn line_span_handles_the_first_and_last_lines() {
        let source = "one\ntwo";
        let (s, e) = line_span(source, 0, 3);
        assert_eq!(&source[s..e], "one\n");
        let (s, e) = line_span(source, 4, 7);
        assert_eq!(&source[s..e], "two");
    }

    #[test]
    fn delete_lines_removes_whole_lines() {
        let source = "keep\ndrop\nkeep2\n";
        assert_eq!(delete_lines(source, vec![(5, 9)]), "keep\nkeep2\n");
    }

    #[test]
    fn delete_lines_deduplicates_two_matches_on_one_line() {
        // Without the dedup this would eat the following line too.
        let source = "a\ntarget target\nb\n";
        let out = delete_lines(source, vec![(2, 8), (9, 15)]);
        assert_eq!(out, "a\nb\n");
    }

    #[test]
    fn delete_lines_with_no_spans_returns_the_input() {
        assert_eq!(delete_lines("abc", Vec::new()), "abc");
    }

    // The tree-sitter-dependent helpers are exercised by every backend's
    // own tests and by the corpus; a grammar dependency here would make
    // this crate depend on a language it has no opinion about.
}
