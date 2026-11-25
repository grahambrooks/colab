//! A `<lang>::pattern` rule's pattern on colab's own matcher: its forms,
//! `where` checks, `let`s and prefilter, compiled once, with what
//! `pattern::PatternRule` asks of it.

use colab_core::Constraint;

use super::{Checks, Forms, Lets};
use crate::pattern::{Found, PATTERN_SLOT, PatternLanguage};

/// A compiled pattern with its constraints.
pub struct RulePattern {
    language: PatternLanguage,
    text: String,
    forms: Forms,
    checks: Checks,
    lets: Lets,
    constraints: Vec<Constraint>,
    prefilter: Option<String>,
}

impl std::fmt::Debug for RulePattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RulePattern({:?}, {} forms)",
            self.text,
            self.forms.forms.len()
        )
    }
}

impl RulePattern {
    /// `text` with its `where` constraints and `let`s compiled for
    /// `language`.
    pub fn compile(
        language: &PatternLanguage,
        text: &str,
        constraints: &[Constraint],
    ) -> Result<Self, String> {
        let forms = super::compile_forms(language, text)?;
        let bound = super::bound(&forms.forms[0].pattern);
        let checks = Checks::compile(language, &bound, constraints)?;
        let lets = Lets::compile(language, &bound, constraints)?;
        let prefilter = super::prefilter(&forms);
        Ok(Self {
            language: language.clone(),
            text: text.to_string(),
            forms,
            checks,
            lets,
            constraints: constraints.to_vec(),
            prefilter,
        })
    }

    pub fn language(&self) -> &PatternLanguage {
        &self.language
    }

    /// The pattern as written.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn constraints(&self) -> &[Constraint] {
        &self.constraints
    }

    pub fn prefilter(&self) -> Option<&str> {
        self.prefilter.as_deref()
    }

    /// Every match in `source`, macro contents' included, sorted by
    /// position, one per span.
    pub fn find(&self, source: &str) -> Vec<Found> {
        let mut found = super::find_where(&self.language, &self.forms, &self.checks, source);
        found.sort_by_key(|f| (f.range.start, f.range.end));
        found.dedup_by_key(|f| (f.range.start, f.range.end));
        // Line and column in one pass over the sorted starts.
        let (mut line, mut line_start, mut at) = (1, 0, 0);
        found
            .into_iter()
            .map(|f| {
                let start = f.range.start;
                for (i, c) in source[at..start].char_indices() {
                    if c == '\n' {
                        line += 1;
                        line_start = at + i + 1;
                    }
                }
                at = start;
                // Some grammars give a node the newline ending it; the
                // match is the code, not that.
                let end = start + source[start..f.range.end].trim_end().len();
                Found {
                    start,
                    end,
                    line,
                    column: source[line_start..start].chars().count() + 1,
                    text: f.text,
                    captures: f.captures,
                }
            })
            .collect()
    }

    /// Every match replaced by `template`, even where the result no
    /// longer parses (the rule refuses that, and explains it).
    pub fn rewrite(&self, source: &str, template: &str) -> String {
        super::rewrite::rewrite_unguarded(
            &self.language,
            &self.forms,
            &self.checks,
            &self.lets,
            source,
            template,
        )
    }

    /// Every match removed, with what belongs to it.
    pub fn delete(&self, source: &str) -> String {
        super::delete(&self.language, &self.forms, &self.checks, source)
    }

    /// `true` when `source` parses without a syntax error.
    pub fn parses_cleanly(&self, source: &str) -> bool {
        super::tree(&self.language.grammar, source).is_some_and(|t| !t.has_error())
    }

    /// Where the first match whose rewrite alone breaks `source` starts,
    /// as `(line, column)`, 1-based.
    pub fn first_breaking_rewrite(&self, source: &str, template: &str) -> Option<(usize, usize)> {
        let at = super::rewrite::first_breaking_edit(
            &self.language,
            &self.forms,
            &self.checks,
            &self.lets,
            source,
            template,
        )?;
        let line = source[..at].matches('\n').count() + 1;
        let line_start = source[..at].rfind('\n').map_or(0, |p| p + 1);
        Some((line, source[line_start..at].chars().count() + 1))
    }

    /// Whether `template`, read as code with its metavariables in place
    /// (in the context the pattern was read in), is a match the rule would
    /// rewrite again: every run would rewrite its own output once more.
    pub fn rematches(&self, template: &str) -> bool {
        let expanded = colab_syntax::pattern::expand(template, self.language.expando);
        let source = match &self.forms.forms[0].context {
            Some(context) => context.replacen(PATTERN_SLOT, &expanded, 1),
            None => expanded,
        };
        !self.find(&source).is_empty() && self.rewrite(&source, template) != source
    }
}
