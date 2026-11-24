//! Finding a pattern: matches in a file's own tree, every form's, that
//! fit the pattern's shape and satisfy its constraints, then those inside
//! macro calls' contents (`macros.rs`).

use super::*;

impl StructuralPattern {
    /// Every match in the tree that satisfies the constraints.
    pub(super) fn matches<'t>(
        &self,
        grep: &'t ast_grep_core::AstGrep<StrDoc<TsLang>>,
        source: &str,
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
            .filter(|m| cheap.iter().all(|c| c.holds(m, source)))
            .collect();
        for check in asked {
            check.prefetch(&found, source);
            found.retain(|m| check.holds(m, source));
        }
        found
    }

    /// Every match in `source` in source order, an outer match before the
    /// matches nested inside it.
    pub fn find(&self, source: &str) -> Vec<Found> {
        let mut found = self.find_file(source);
        // Matches inside macro calls' contents, mapped back into `source`.
        let mut lines = Lines::new(source);
        for region in self.macro_regions(source) {
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
    pub(super) fn find_file(&self, source: &str) -> Vec<Found> {
        let mut found = self.find_own(source);
        for alt in &self.also {
            found.extend(alt.find_own(source));
        }
        found.sort_by_key(|f| (f.start, f.end));
        found.dedup_by_key(|f| (f.start, f.end));
        found
    }

    /// Matches of this compilation only, not its `also` ones.
    fn find_own(&self, source: &str) -> Vec<Found> {
        let grep = parsed(&self.lang, source);
        let mut lines = Lines::new(source);
        self.matches(&grep, source)
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
