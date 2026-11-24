//! Matching inside macro calls' contents, which the grammar keeps as
//! token trees (see [`MacroContents`]): each listed call's contents are
//! parsed in the language's macro wrapper, matched there, and the matches
//! and edits mapped back into the file.

use super::*;

/// A macro call's contents, as [`StructuralPattern::find`] reads them.
pub(super) struct MacroRegion {
    /// The contents in the source: inside the call's delimiters.
    pub(super) inner: std::ops::Range<usize>,
    /// Where the contents start in `wrapped`.
    pub(super) offset: usize,
    /// The contents in the language's macro wrapper.
    pub(super) wrapped: String,
    /// The pattern's matches in `wrapped`, all within the contents.
    pub(super) found: Vec<Found>,
}

impl StructuralPattern {
    /// The listed macro calls in `source` whose contents parse in the
    /// language's macro wrapper and hold a match, outermost calls only (a
    /// call inside another's contents is found when those are read), each
    /// with its matches in the wrapped text (nested macros' included).
    /// Empty when the language reads no macro contents, or a constraint
    /// reads a match's surroundings.
    pub(super) fn macro_regions(&self, source: &str) -> Vec<MacroRegion> {
        let Some(spec) = self.lang.macro_contents else {
            return Vec::new();
        };
        if self.checks.iter().any(Check::reads_context)
            || self
                .prefilter
                .as_deref()
                .is_some_and(|literal| !source.contains(literal))
        {
            return Vec::new();
        }
        let Some(slot) = spec.wrapper.find(PATTERN_SLOT) else {
            return Vec::new();
        };
        let Some(tree) = crate::parse_cache::parse(&self.lang.grammar, source) else {
            return Vec::new();
        };
        let mut regions = Vec::new();
        crate::visit_all(&tree, |node| {
            if node.kind() != spec.invocation {
                return;
            }
            let named = node
                .child_by_field_name(spec.name_field)
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(|name| name.rsplit("::").next().unwrap_or(name).trim());
            if !named.is_some_and(|name| spec.macros.contains(&name)) {
                return;
            }
            let Some(body) = node
                .named_child(
                    u32::try_from(node.named_child_count().saturating_sub(1)).unwrap_or(u32::MAX),
                )
                .filter(|b| b.kind() == spec.body && b.end_byte() - b.start_byte() >= 2)
            else {
                return;
            };
            // Inside the delimiters: `(`…`)`, `[`…`]`, `{`…`}`.
            let inner = body.start_byte() + 1..body.end_byte() - 1;
            let contents = &source[inner.clone()];
            if self
                .prefilter
                .as_deref()
                .is_some_and(|literal| !contents.contains(literal))
            {
                return;
            }
            let wrapped = spec.wrapper.replacen(PATTERN_SLOT, contents, 1);
            if parse_clean(&self.lang.grammar, &wrapped).is_err() {
                return;
            }
            let found = self.find(&wrapped);
            let within = slot..slot + contents.len();
            // A match reaching into the wrapper (`$F($X)` matching the
            // wrapper's own call) would make an edit outside the macro;
            // such contents are left alone, so find and rewrite agree.
            if found.is_empty()
                || found
                    .iter()
                    .any(|f| f.start < within.start || f.end > within.end)
            {
                return;
            }
            regions.push(MacroRegion {
                inner,
                offset: slot,
                wrapped,
                found,
            });
        });
        regions
    }

    /// `edit` (a rewrite or delete of whole text) applied to each macro
    /// region of `source`: the region's wrapped contents edited, and the
    /// result's contents put back where they came from. A region whose
    /// wrapper the edit changed is left as it was.
    pub(super) fn edit_macros(&self, source: &str, edit: impl Fn(&str) -> String) -> String {
        let regions = self.macro_regions(source);
        if regions.is_empty() {
            return source.to_string();
        }
        let mut out = source.to_string();
        for region in regions.iter().rev() {
            let changed = edit(&region.wrapped);
            let suffix = region.wrapped.len() - region.offset - region.inner.len();
            let (head, tail) = (
                &region.wrapped[..region.offset],
                &region.wrapped[region.wrapped.len() - suffix..],
            );
            if changed == region.wrapped
                || changed.len() < head.len() + tail.len()
                || !changed.starts_with(head)
                || !changed.ends_with(tail)
            {
                continue;
            }
            out.replace_range(
                region.inner.clone(),
                &changed[head.len()..changed.len() - tail.len()],
            );
        }
        out
    }
}
