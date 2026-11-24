//! `PatternRule`: a compiled `<lang>::pattern` rule as an `Operation` —
//! replace or delete, its prefilter, conflicts, near misses and advice.

use super::*;

/// What a `<lang>::pattern` rule does with each match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternAction {
    /// Replace the match with this template (`$NAME` / `$$$NAME`
    /// substituted).
    Replace(String),
    /// Remove the match (with its line, when it stands alone).
    Delete,
}

/// A compiled pattern plus its action: everything a backend's
/// `<lang>::pattern` operation does except decide which files it applies
/// to and print its namespace.
#[derive(Debug)]
pub struct PatternRule {
    pub pattern: StructuralPattern,
    pub action: PatternAction,
    /// How this language spells a qualified name, for
    /// [`conflicts`](PatternRule::conflicts). `None` skips that check.
    pub qualifiers: Option<Qualifiers>,
}

/// How a language spells a qualified reference such as `fmt.Errorf`, and
/// which qualifiers a file's imports bind. With it, a rewrite that
/// introduces a qualifier nothing imports is reported as a conflict.
#[derive(Clone, Copy)]
pub struct Qualifiers {
    /// The node kind of a qualified reference: `selector_expression` in
    /// Go, `attribute` in Python.
    pub kind: &'static str,
    /// The field holding the qualifier: `operand` in Go, `object` in
    /// Python. Only a qualifier that is a single leaf is checked.
    pub field: &'static str,
    /// Every name a file's imports bind, alias or conventional name. `*`
    /// means "cannot tell" (a wildcard import), which silences the check.
    pub imported: fn(&str) -> Vec<String>,
    /// Qualifiers that need no import: builtins such as Python's `str`.
    pub builtin: &'static [&'static str],
}

/// `true` when `template` spells `name.` with `name` as a whole word, so
/// `fmt.Errorf` qualifies by `fmt` but `xfmt.F` does not.
pub(super) fn template_qualifies(template: &str, name: &str) -> bool {
    let dotted = format!("{name}.");
    template.match_indices(&dotted).any(|(at, _)| {
        !template[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
    })
}

impl std::fmt::Debug for Qualifiers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Qualifiers({}.{})", self.kind, self.field)
    }
}

impl PatternRule {
    /// Compile `text` with its `where` constraints (`&[]` for none).
    pub fn compile(
        language: &PatternLanguage,
        text: &str,
        action: PatternAction,
        constraints: &[Constraint],
    ) -> Result<Self, String> {
        if matches!(action, PatternAction::Delete)
            && let Some(c) = constraints.iter().find(|c| c.is_let())
        {
            return Err(format!(
                "`{c}`: a `let` derives a value for the `replace` template, and `delete` has none"
            ));
        }
        if let PatternAction::Replace(template) = &action {
            unbound_in_template(text, template, constraints)?;
        }
        Ok(Self {
            pattern: StructuralPattern::compile(language, text)?
                .constrain(language, constraints)?,
            action,
            qualifiers: None,
        })
    }

    /// Check rewrites for qualifiers nothing imports (see [`Qualifiers`]).
    pub fn with_qualifiers(mut self, qualifiers: Qualifiers) -> Self {
        self.qualifiers = Some(qualifiers);
        self
    }

    /// Why [`apply`](PatternRule::apply) leaves `source` unchanged although
    /// the pattern matches there: the rewritten file would not parse.
    /// A warning when the template, read as code with its metavariables
    /// in place, is itself a match: every run would rewrite its own output
    /// again (`f($A)` → `f(g($A))`). An identity template is a no-op and
    /// is not flagged.
    pub fn advice(&self) -> Vec<String> {
        let PatternAction::Replace(template) = &self.action else {
            return Vec::new();
        };
        if template.trim() == self.pattern.text().trim() || !self.pattern.rematches(template) {
            return Vec::new();
        }
        vec![
            "the template matches the pattern again, so every run rewrites its own output once \
             more; narrow the pattern (a `where` clause, or a name the template changes), or apply \
             it once and remove the rule"
                .into(),
        ]
    }

    pub fn why_unchanged(&self, source: &str) -> Option<String> {
        let PatternAction::Replace(template) = &self.action else {
            return None;
        };
        let out = self.pattern.rewrite(source, template);
        if out == source && !self.pattern.find(source).is_empty() {
            return Some(
                "the template gives back what it matched, apart from whitespace, so nothing was \
                 changed"
                    .into(),
            );
        }
        if out == source
            || self.pattern.parses_cleanly(&out)
            || !self.pattern.parses_cleanly(source)
        {
            return None;
        }
        // Name the match that breaks it, when one does on its own: in a
        // file of many matches, the one shaped differently (a struct
        // literal ending in `..base`) is otherwise hard to find.
        let site = self
            .pattern
            .first_breaking_rewrite(source, template)
            .map_or_default(|(line, column)| {
                format!(" (the rewrite of the match at line {line}, column {column} breaks it)")
            });
        Some(format!(
            "the rewritten code would not parse, so it was skipped{site}; check the template \
             (a `$$$` capture keeps its separators, so `$$$ARGS,` doubles a comma)"
        ))
    }

    /// Qualifiers this rule's template introduced that the file neither
    /// used `before` nor imports `after` (the file as the whole script
    /// leaves it): `fmt.` in a template, with no `requires import "fmt"`
    /// and no later rule adding it. Syntactic, so it errs towards silence:
    /// a qualifier already present anywhere in `before`, as any leaf,
    /// counts as known.
    pub fn conflicts(&self, before: &str, after: &str) -> Vec<String> {
        let (Some(q), PatternAction::Replace(template)) = (self.qualifiers, &self.action) else {
            return Vec::new();
        };
        let grammar = &self.pattern.lang.grammar;
        let (Ok(old), Ok(new)) = (parse_clean(grammar, before), parse_clean(grammar, after)) else {
            return Vec::new();
        };
        let mut known: std::collections::HashSet<&str> = std::collections::HashSet::new();
        crate::visit_all(&old, |node| {
            if node.child_count() == 0 {
                known.insert(&before[node.byte_range()]);
            }
        });
        let imported = (q.imported)(after);
        let mut introduced: Vec<String> = Vec::new();
        crate::visit_all(&new, |node| {
            if node.kind() != q.kind {
                return;
            }
            let Some(qualifier) = node.child_by_field_name(q.field) else {
                return;
            };
            if qualifier.child_count() != 0 {
                return;
            }
            let name = &after[qualifier.byte_range()];
            // Only a qualifier this rule's own template spells is its doing;
            // `after` may hold later rules' changes too.
            if template_qualifies(template, name)
                && !known.contains(name)
                && !q.builtin.contains(&name)
                && !imported.iter().any(|i| i == "*" || i == name)
                && !introduced.iter().any(|i| i == name)
            {
                introduced.push(name.to_string());
            }
        });
        introduced
            .into_iter()
            .map(|name| {
                format!(
                    "the rewrite uses `{name}.`, which this file does not import; \
                     add `requires import \"…\"` to the rule"
                )
            })
            .collect()
    }

    /// Rewrite `source`. If it parsed cleanly and the rewrite would leave
    /// a syntax error — a template that is not valid code where it lands —
    /// the file is returned unchanged rather than written broken. The rule
    /// then reports no change for that file, while `find` still lists its
    /// matches: that gap is the signal to fix the template.
    pub fn apply(&self, source: &str) -> String {
        let out = match &self.action {
            PatternAction::Replace(template) => self.pattern.rewrite(source, template),
            PatternAction::Delete => self.pattern.delete(source),
        };
        if out != source
            && !self.pattern.parses_cleanly(&out)
            && self.pattern.parses_cleanly(source)
        {
            return source.to_string();
        }
        out
    }

    pub fn prefilter(&self) -> Option<&str> {
        self.pattern.prefilter()
    }

    /// Every match in `source`, for read-only search.
    pub fn find(&self, source: &str) -> Vec<Found> {
        self.pattern.find(source)
    }
}

impl std::fmt::Display for PatternRule {
    /// `pattern 'errors.Wrap($E, $M)' -> 'fmt.Errorf(...)'`, without the
    /// language prefix, which the backend adds.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pattern '{}'{}{}",
            self.pattern.text(),
            colab_core::constraint::display_where(self.pattern.constraints()),
            colab_core::constraint::display_lets(self.pattern.constraints())
        )?;
        match &self.action {
            PatternAction::Replace(template) => write!(f, " -> '{template}'"),
            PatternAction::Delete => write!(f, " -> delete"),
        }
    }
}
