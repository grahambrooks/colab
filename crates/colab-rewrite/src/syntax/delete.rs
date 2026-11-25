//! Deleting colab's own matches, with what belongs to them (the helpers
//! are `pattern/delete.rs`'s): a match alone on its
//! line(s) goes with the whole line; an item's doc comments and attributes
//! go with it; a list element goes with the `,` beside it; a match with no
//! comma beside it inside a line is left alone.

use super::{Checks, Forms, Frame};
use crate::line_span;
use crate::pattern::{
    PatternLanguage, attached_start, own_start, with_separator, without_double_blank,
};

/// `source` with every match of `forms` that `checks` hold for removed,
/// then inside the macro calls the language reads.
pub fn delete(language: &PatternLanguage, forms: &Forms, checks: &Checks, source: &str) -> String {
    delete_in(language, forms, checks, source, None, 0)
}

fn delete_in(
    language: &PatternLanguage,
    forms: &Forms,
    checks: &Checks,
    source: &str,
    frame: Option<&Frame<'_>>,
    depth: usize,
) -> String {
    let Some(tree) = super::source_tree(language, source) else {
        return source.to_string();
    };
    let leading = language.leading;
    let attaches = !(leading.kinds.is_empty() && leading.doc_comments.is_empty());
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for found in super::find_in_tree(language, forms, checks, &tree, frame) {
        // A node that owns the newline ending it is the code before it.
        let (start, end) = (found.range.start, found.range.end);
        let end = start + source[start..end].trim_end().len();
        // An item's doc comments and attributes go with it.
        // From the item's own first token (a tree may hold comments that
        // are not its own), back over what belongs to it.
        let first = if attaches {
            let own = own_start(&tree, start, end, leading);
            attached_start(&tree, source, own, end, leading)
        } else {
            start
        };
        let (line_start, line_end) = line_span(source, first, end);
        let before = &source[line_start..first];
        let after = &source[end..line_end];
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
        if let Some(span) = with_separator(&tree, start, end) {
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
    // Then inside macro calls, where the wrapper's tree says which comma
    // goes with an argument.
    let moved = frame.map(|f| f.resized(source.len(), out.len()));
    super::edit_macros(
        language,
        forms,
        checks,
        &out,
        moved.as_ref(),
        depth,
        |wrapped, inner, d| delete_in(language, forms, checks, wrapped, Some(inner), d),
    )
}
