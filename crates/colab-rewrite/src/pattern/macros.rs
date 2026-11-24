//! Matching inside macro calls' contents, which the grammar keeps as
//! token trees (see [`MacroContents`]): each listed call's contents are
//! parsed in the language's macro wrapper, matched there, and the matches
//! and edits mapped back into the file.
//!
//! A match in the contents is judged as one in the file would be
//! ([`Frame`]): what encloses it is the contents' own nodes and then the
//! macro call and its ancestors in the file, never the wrapper; its
//! neighbours are the contents' own; and a language server is asked about
//! the file at the position the contents came from.

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

/// Whether an enclosing node matches a nested pattern (`inside`'s, or an
/// `until`'s). The node itself stays in the tree it came from, so frames
/// over trees of different lifetimes nest.
pub(super) type Enclosing<'f> = Box<dyn Fn(&check::Forms) -> bool + 'f>;

/// Where a macro's contents, re-parsed in the wrapper, sit in the file:
/// what a `where` constraint on a match there reads instead of the
/// wrapper around it.
pub(super) struct Frame<'f> {
    /// The wrapper's text before the contents, and after them: a node
    /// outside `head..len - tail` is the wrapper's.
    head: usize,
    tail: usize,
    /// The wrapped text's length now (edits to the contents change it).
    len: usize,
    /// The macro call and its ancestors, nearest first, in the file's tree
    /// (and, for a nested macro, the enclosing contents' first): a match's
    /// ancestors beyond the contents.
    pub(super) outside: &'f [Enclosing<'f>],
    /// The file's text, which a language server is asked about.
    pub(super) file: &'f str,
    /// A position in the wrapped text as it is now, as one in `file`.
    map: Box<dyn Fn(usize) -> usize + 'f>,
}

impl Frame<'_> {
    /// Whether `node` is the contents' own, not the wrapper's.
    pub(super) fn holds(&self, node: &check::TsNode<'_>) -> bool {
        let range = node.range();
        range.start >= self.head && range.end <= self.len.saturating_sub(self.tail)
    }

    /// `span` of the wrapped text, as a span of the file.
    pub(super) fn to_file(&self, span: std::ops::Range<usize>) -> std::ops::Range<usize> {
        (self.map)(span.start)..(self.map)(span.end)
    }

    /// The frame of the same contents after `edits` — `(start, end, new
    /// length)` in the text as it was, in order and disjoint — made it
    /// `len` long: a later stage's matches are then judged, and asked
    /// about, where they are now. A position inside an edit maps to the
    /// edit's start.
    pub(super) fn after(&self, edits: Vec<(usize, usize, usize)>, len: usize) -> Frame<'_> {
        let back = move |p: usize| {
            let mut shift: isize = 0;
            for &(start, end, new_len) in &edits {
                let new_start = start.saturating_add_signed(shift);
                if p < new_start {
                    break;
                }
                if p < new_start + new_len {
                    return start;
                }
                shift += new_len as isize - (end - start) as isize;
            }
            p.saturating_add_signed(-shift)
        };
        Frame {
            head: self.head,
            tail: self.tail,
            len,
            outside: self.outside,
            file: self.file,
            map: Box::new(move |p| (self.map)(back(p))),
        }
    }
}

/// The macros a run reads beyond the language's own: those the project
/// lists (`colab.toml` `[patterns] macros`), set on the current
/// [`Run`](colab_core::run::Run).
fn user_macros() -> Vec<String> {
    colab_core::run::Run::current().map_or_else(Vec::new, |run| run.macros())
}

impl StructuralPattern {
    /// The listed macro calls in `source` whose contents parse in the
    /// language's macro wrapper and hold a match, outermost calls only (a
    /// call inside another's contents is found when those are read), each
    /// with its matches in the wrapped text (nested macros' included) and
    /// what `then` makes of it, given the region's [`Frame`]. `frame` is
    /// the one `source` itself is in, when it is a macro's contents.
    /// Empty when the language reads no macro contents.
    pub(super) fn macro_regions<R>(
        &self,
        source: &str,
        frame: Option<&Frame<'_>>,
        mut then: impl FnMut(&MacroRegion, &Frame<'_>) -> R,
    ) -> Vec<(MacroRegion, R)> {
        let Some(spec) = self.lang.macro_contents else {
            return Vec::new();
        };
        if self
            .prefilter
            .as_deref()
            .is_some_and(|literal| !source.contains(literal))
        {
            return Vec::new();
        }
        let Some(slot) = spec.wrapper.find(PATTERN_SLOT) else {
            return Vec::new();
        };
        let extra = user_macros();
        let listed =
            |name: &str| spec.macros.contains(&name) || extra.iter().any(|m| m.as_str() == name);
        let grep = parsed(&self.lang, source);
        let root = grep.root();
        let mut regions = Vec::new();
        for node in root.dfs() {
            if node.kind() != spec.invocation || frame.is_some_and(|f| !f.holds(&node)) {
                continue;
            }
            let named = node.field(spec.name_field).map(|n| n.text().into_owned());
            let name = named
                .as_deref()
                .map(|name| name.rsplit("::").next().unwrap_or(name).trim());
            if !name.is_some_and(listed) {
                continue;
            }
            let Some(body) = node
                .children()
                .filter(ast_grep_core::Node::is_named)
                .last()
                .filter(|b| b.kind() == spec.body && b.range().len() >= 2)
            else {
                continue;
            };
            // Inside the delimiters: `(`…`)`, `[`…`]`, `{`…`}`.
            let inner = body.range().start + 1..body.range().end - 1;
            let contents = &source[inner.clone()];
            if self
                .prefilter
                .as_deref()
                .is_some_and(|literal| !contents.contains(literal))
            {
                continue;
            }
            let wrapped = spec.wrapper.replacen(PATTERN_SLOT, contents, 1);
            if parse_clean(&self.lang.grammar, &wrapped).is_err() {
                continue;
            }
            // Beyond the contents: this call and its ancestors (within the
            // enclosing contents, for a nested call), then the enclosing
            // frame's.
            let mut outside: Vec<Enclosing<'_>> = std::iter::once(node.clone())
                .chain(node.ancestors())
                .filter(|a| frame.is_none_or(|f| f.holds(a)))
                .map(|a| -> Enclosing<'_> { Box::new(move |forms| forms.match_node(&a)) })
                .collect();
            if let Some(f) = frame {
                outside.extend(
                    f.outside
                        .iter()
                        .map(|outer| -> Enclosing<'_> { Box::new(move |forms| outer(forms)) }),
                );
            }
            let within = slot..slot + contents.len();
            let base = inner.start;
            let map: Box<dyn Fn(usize) -> usize + '_> = match frame {
                Some(f) => Box::new(move |p: usize| (f.map)(p.saturating_sub(slot) + base)),
                None => Box::new(move |p: usize| p.saturating_sub(slot) + base),
            };
            let inner_frame = Frame {
                head: slot,
                tail: wrapped.len() - within.end,
                len: wrapped.len(),
                outside: &outside,
                file: frame.map_or(source, |f| f.file),
                map,
            };
            let found = self.find_in(&wrapped, Some(&inner_frame));
            // A match reaching into the wrapper (`$F($X)` matching the
            // wrapper's own call) would make an edit outside the macro;
            // such contents are left alone, so find and rewrite agree.
            if found.is_empty()
                || found
                    .iter()
                    .any(|f| f.start < within.start || f.end > within.end)
            {
                continue;
            }
            let region = MacroRegion {
                inner,
                offset: slot,
                wrapped,
                found,
            };
            let made = then(&region, &inner_frame);
            regions.push((region, made));
        }
        regions
    }

    /// `edit` (a rewrite or delete of whole text, in the region's frame)
    /// applied to each macro region of `source`: the region's wrapped
    /// contents edited, and the result's contents put back where they came
    /// from. A region whose wrapper the edit changed is left as it was.
    pub(super) fn edit_macros(
        &self,
        source: &str,
        frame: Option<&Frame<'_>>,
        edit: impl Fn(&str, &Frame<'_>) -> String,
    ) -> String {
        let regions =
            self.macro_regions(source, frame, |region, inner| edit(&region.wrapped, inner));
        if regions.is_empty() {
            return source.to_string();
        }
        let mut out = source.to_string();
        for (region, changed) in regions.iter().rev() {
            let suffix = region.wrapped.len() - region.offset - region.inner.len();
            let (head, tail) = (
                &region.wrapped[..region.offset],
                &region.wrapped[region.wrapped.len() - suffix..],
            );
            if *changed == region.wrapped
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
