//! Finding a pattern: matches in a file's own tree, every form's, that
//! fit the pattern's shape and satisfy its constraints, then those inside
//! macro calls' contents (`macros.rs`).

use super::*;

impl StructuralPattern {
    /// Every match in the tree that satisfies the constraints, judged in
    /// `frame` when `source` is a macro's contents.
    pub(super) fn matches<'t>(
        &self,
        grep: &'t ast_grep_core::AstGrep<StrDoc<TsLang>>,
        source: &str,
        frame: Option<&Frame<'_>>,
    ) -> Vec<NodeMatch<'t, StrDoc<TsLang>>> {
        // Semantic checks are sorted last. Each one asks the server about
        // every match still standing in one batch, so a file's questions
        // cost one round trip per check rather than one per match.
        let (cheap, asked) = self
            .checks
            .split_at(self.checks.partition_point(|c| !c.is_semantic()));
        let mut found: Vec<_> = candidates(grep, source, &self.pattern)
            .into_iter()
            .filter(|m| self.shape.fits(m.get_node()))
            .filter_map(|mut m| {
                self.visible
                    .as_ref()
                    .is_none_or(|v| v.bound(&mut m))
                    .then_some(m)
            })
            .filter(|m| cheap.iter().all(|c| c.holds(m, source, frame)))
            .collect();
        for check in asked {
            check.prefetch(&found, source, frame);
            found.retain(|m| check.holds(m, source, frame));
        }
        found
    }

    /// Every match in `source` in source order, an outer match before the
    /// matches nested inside it.
    pub fn find(&self, source: &str) -> Vec<Found> {
        self.find_in(source, None)
    }

    /// [`find`](Self::find), judged in `frame` when `source` is a macro's
    /// contents.
    pub(super) fn find_in(&self, source: &str, frame: Option<&Frame<'_>>) -> Vec<Found> {
        let mut found = self.find_file(source, frame);
        // Matches inside macro calls' contents, mapped back into `source`.
        let mut lines = Lines::new(source);
        for (region, ()) in self.macro_regions(source, frame, |_, _| ()) {
            for mut hit in region.found {
                hit.start = hit.start - region.offset + region.inner.start;
                hit.end = hit.end - region.offset + region.inner.start;
                (hit.line, hit.column) = lines.at(hit.start);
                found.push(hit);
            }
        }
        found.sort_by_key(|f| (f.start, f.end));
        found.dedup_by_key(|f| (f.start, f.end));
        found
    }

    /// Matches in the file's own tree, every form's, none inside a macro.
    pub(super) fn find_file(&self, source: &str, frame: Option<&Frame<'_>>) -> Vec<Found> {
        let mut found = self.find_own(source, frame);
        for alt in &self.also {
            found.extend(alt.find_own(source, frame));
        }
        found.sort_by_key(|f| (f.start, f.end));
        found.dedup_by_key(|f| (f.start, f.end));
        found
    }

    /// Matches of this compilation only, not its `also` ones.
    fn find_own(&self, source: &str, frame: Option<&Frame<'_>>) -> Vec<Found> {
        let grep = parsed(&self.lang, source);
        let mut lines = Lines::new(source);
        self.matches(&grep, source, frame)
            .into_iter()
            .map(|m| {
                let mut range = m.range();
                // Some grammars give a node the newline that ends it (Go's
                // `expression_case`); the match is the code, not that.
                range.end = range.start + source[range.clone()].trim_end().len();
                let (line, column) = lines.at(range.start);
                let env = m.get_env();
                let mut captures: Vec<(String, String)> = env
                    .get_matched_variables()
                    .filter_map(|var| match var {
                        // An item's modifiers bound by the form that reads
                        // them: colab's, not the pattern's.
                        MetaVariable::Capture(name, _) | MetaVariable::MultiCapture(name)
                            if name == MODIFIERS =>
                        {
                            None
                        }
                        MetaVariable::Capture(name, _) => env
                            .get_match(&name)
                            .map(|node| (name.clone(), node.text().into_owned())),
                        MetaVariable::MultiCapture(name) => {
                            let nodes = env.get_multiple_matches(&name);
                            let text = match (nodes.first(), nodes.last()) {
                                (Some(first), Some(last)) => {
                                    source[first.range().start..last.range().end].to_string()
                                }
                                _ => String::new(),
                            };
                            Some((name, text))
                        }
                        _ => None,
                    })
                    .collect();
                captures.sort();
                captures.dedup();
                Found {
                    start: range.start,
                    end: range.end,
                    line,
                    column,
                    text: m.text().into_owned(),
                    captures,
                }
            })
            .collect()
    }
}
