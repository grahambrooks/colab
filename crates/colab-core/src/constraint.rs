//! `where` constraints on a `<lang>::pattern` rule, as plain data.
//!
//! ```text
//! match go::pattern 'errors.Wrap($E, $M)'
//!     where $M matches /^"/, not inside 'func Test$_($$$) { $$$ }'
//!     { replace 'fmt.Errorf($M + ": %w", $E)' }
//! ```
//!
//! The DSL parses these into [`Constraint`]s and hands them to the backend
//! through [`LanguageBackend::build_rule_where`](crate::LanguageBackend::build_rule_where).
//! Evaluation lives in `colab_rewrite::pattern`. A constraint only removes
//! matches, never adds one, so a pattern's prefilter stays sound.

use std::fmt;

/// A metavariable named in a constraint: `$NAME`, or `$$$NAME` for a
/// sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    /// The name without its `$` / `$$$` sigil.
    pub name: String,
    /// `true` for `$$$NAME`.
    pub multi: bool,
}

impl Var {
    /// Parse `$NAME` / `$$$NAME` as written in a script.
    pub fn parse(text: &str) -> Option<Self> {
        let (multi, name) = match text.strip_prefix("$$$") {
            Some(name) => (true, name),
            None => (false, text.strip_prefix('$')?),
        };
        let valid = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        valid.then(|| Self {
            name: name.to_string(),
            multi,
        })
    }
}

impl fmt::Display for Var {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sigil = if self.multi { "$$$" } else { "$" };
        write!(f, "{sigil}{}", self.name)
    }
}

/// What one constraint checks about a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Test {
    /// `$X matches /re/`: the binding's text matches the regular
    /// expression, unanchored.
    Matches { var: Var, regex: String },
    /// `$X is <kind>`: the bound node has this tree-sitter kind.
    Is { var: Var, kind: String },
    /// `$$$X count n..m`: the sequence binds between `min` and `max`
    /// named nodes, both inclusive. `max: None` is unbounded.
    Count {
        var: Var,
        min: usize,
        max: Option<usize>,
    },
    /// `inside '<pattern>'`: some ancestor of the match matches it.
    Inside(String),
    /// `has '<pattern>'`: some descendant of the match matches it.
    Has(String),
}

/// One `where` constraint, optionally negated with `not`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constraint {
    pub negated: bool,
    pub test: Test,
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negated {
            f.write_str("not ")?;
        }
        match &self.test {
            Test::Matches { var, regex } => write!(f, "{var} matches /{regex}/"),
            Test::Is { var, kind } => write!(f, "{var} is {kind}"),
            Test::Count { var, min, max } => match max {
                Some(max) if max == min => write!(f, "{var} count {min}"),
                Some(max) => write!(f, "{var} count {min}..{max}"),
                None => write!(f, "{var} count {min}.."),
            },
            Test::Inside(pattern) => write!(f, "inside '{pattern}'"),
            Test::Has(pattern) => write!(f, "has '{pattern}'"),
        }
    }
}

/// `where a, b, c` as it reads in a script, or `""` for no constraints.
pub fn display_where(constraints: &[Constraint]) -> String {
    if constraints.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = constraints.iter().map(ToString::to_string).collect();
    format!(" where {}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_sigils_and_rejects_other_text() {
        assert_eq!(
            Var::parse("$MSG"),
            Some(Var {
                name: "MSG".into(),
                multi: false
            })
        );
        assert_eq!(
            Var::parse("$$$ARGS"),
            Some(Var {
                name: "ARGS".into(),
                multi: true
            })
        );
        assert_eq!(Var::parse("$msg"), None);
        assert_eq!(Var::parse("MSG"), None);
        assert_eq!(Var::parse("$$A"), None);
    }

    #[test]
    fn displays_as_written() {
        let var = Var::parse("$$$A").unwrap();
        let c = |negated, test| Constraint { negated, test };
        let all = [
            c(
                false,
                Test::Matches {
                    var: Var::parse("$M").unwrap(),
                    regex: "^\"".into(),
                },
            ),
            c(true, Test::Inside("func Test$_($$$) { $$$ }".into())),
            c(
                false,
                Test::Count {
                    var: var.clone(),
                    min: 1,
                    max: None,
                },
            ),
            c(
                false,
                Test::Count {
                    var,
                    min: 2,
                    max: Some(2),
                },
            ),
        ];
        assert_eq!(
            display_where(&all),
            " where $M matches /^\"/, not inside 'func Test$_($$$) { $$$ }', $$$A count 1.., $$$A count 2"
        );
        assert_eq!(display_where(&[]), "");
    }
}
