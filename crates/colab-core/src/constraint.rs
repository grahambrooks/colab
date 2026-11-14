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
    /// `let $NEW = $FROM | step | …` in a rule's action block: not a
    /// test but a value derived from a binding, for the `replace`
    /// template. It narrows nothing, so it is carried with the
    /// constraints only to reach the pattern compiler unchanged.
    Let {
        var: Var,
        from: Var,
        steps: Vec<Transform>,
    },
}

/// One step of a `let` pipeline: what it does to the text it is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transform {
    /// `lower` / `upper`: the whole text's case.
    Lower,
    Upper,
    /// `capitalize`: the first character upper-cased, the rest kept.
    Capitalize,
    /// `snake`, `kebab`, `camel`, `pascal`, `upper_snake`: the text split
    /// into words (at `_`, `-`, spaces and case changes) and joined again.
    Snake,
    Kebab,
    Camel,
    Pascal,
    UpperSnake,
    /// `strip_prefix("Get")` / `strip_suffix("Impl")`: removed when
    /// present, otherwise the text is kept.
    StripPrefix(String),
    StripSuffix(String),
    /// `replace(/re/, "with")`: every match replaced; `$1` / `${name}` in
    /// `with` name the regex's groups.
    Replace {
        regex: String,
        with: String,
    },
    /// `substring(start, end)`: characters `start..end`; a negative index
    /// counts from the end, and `end` may be left out.
    Substring {
        start: i64,
        end: Option<i64>,
    },
}

/// An argument to a transform, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransformArg {
    Text(String),
    Regex(String),
    Int(i64),
}

impl fmt::Display for TransformArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(t) => write!(f, "\"{t}\""),
            Self::Regex(r) => write!(f, "/{r}/"),
            Self::Int(n) => write!(f, "{n}"),
        }
    }
}

/// The transforms a `let` takes, as `name(args)` for messages.
pub const TRANSFORMS: &[&str] = &[
    "lower",
    "upper",
    "capitalize",
    "snake",
    "kebab",
    "camel",
    "pascal",
    "upper_snake",
    "strip_prefix",
    "strip_suffix",
    "replace",
    "substring",
];

impl Transform {
    /// The step `name(args)`, or why it is not one.
    pub fn parse(name: &str, args: Vec<TransformArg>) -> Result<Self, String> {
        use TransformArg::{Int, Regex, Text};
        let shown = || {
            let list: Vec<String> = args.iter().map(ToString::to_string).collect();
            if list.is_empty() {
                name.to_string()
            } else {
                format!("{name}({})", list.join(", "))
            }
        };
        let plain = |t: Self| {
            if args.is_empty() {
                Ok(t)
            } else {
                Err(format!("`{}`: `{name}` takes no arguments", shown()))
            }
        };
        match name {
            "lower" => plain(Self::Lower),
            "upper" => plain(Self::Upper),
            "capitalize" => plain(Self::Capitalize),
            "snake" => plain(Self::Snake),
            "kebab" => plain(Self::Kebab),
            "camel" => plain(Self::Camel),
            "pascal" => plain(Self::Pascal),
            "upper_snake" => plain(Self::UpperSnake),
            "strip_prefix" | "strip_suffix" => match args.as_slice() {
                [Text(t)] if name == "strip_prefix" => Ok(Self::StripPrefix(t.clone())),
                [Text(t)] => Ok(Self::StripSuffix(t.clone())),
                _ => Err(format!(
                    "`{}`: `{name}` takes one string, `{name}(\"…\")`",
                    shown()
                )),
            },
            "replace" => match args.as_slice() {
                [Regex(regex) | Text(regex), Text(with)] => Ok(Self::Replace {
                    regex: regex.clone(),
                    with: with.clone(),
                }),
                _ => Err(format!(
                    "`{}`: `replace` takes a regex and a string, `replace(/re/, \"with\")`",
                    shown()
                )),
            },
            "substring" => match args.as_slice() {
                [Int(start)] => Ok(Self::Substring {
                    start: *start,
                    end: None,
                }),
                [Int(start), Int(end)] => Ok(Self::Substring {
                    start: *start,
                    end: Some(*end),
                }),
                _ => Err(format!(
                    "`{}`: `substring` takes one or two integers, `substring(start, end)`",
                    shown()
                )),
            },
            _ => {
                let hint = crate::suggest::closest(name, TRANSFORMS.iter().copied())
                    .map(|t| format!(" (did you mean `{t}`?)"))
                    .unwrap_or_default();
                Err(format!(
                    "unknown transform `{name}`{hint}; a `let` takes {}",
                    TRANSFORMS
                        .iter()
                        .map(|t| format!("`{t}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
    }
}

impl fmt::Display for Transform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lower => f.write_str("lower"),
            Self::Upper => f.write_str("upper"),
            Self::Capitalize => f.write_str("capitalize"),
            Self::Snake => f.write_str("snake"),
            Self::Kebab => f.write_str("kebab"),
            Self::Camel => f.write_str("camel"),
            Self::Pascal => f.write_str("pascal"),
            Self::UpperSnake => f.write_str("upper_snake"),
            Self::StripPrefix(t) => write!(f, "strip_prefix(\"{t}\")"),
            Self::StripSuffix(t) => write!(f, "strip_suffix(\"{t}\")"),
            Self::Replace { regex, with } => write!(f, "replace(/{regex}/, \"{with}\")"),
            Self::Substring { start, end: None } => write!(f, "substring({start})"),
            Self::Substring {
                start,
                end: Some(end),
            } => write!(f, "substring({start}, {end})"),
        }
    }
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
            Test::Let { var, from, steps } => {
                write!(f, "let {var} = {from}")?;
                for step in steps {
                    write!(f, " | {step}")?;
                }
                Ok(())
            }
        }
    }
}

/// `where a, b, c` as it reads in a script, or `""` for no constraints
/// (`let` clauses are left to [`display_lets`]).
pub fn display_where(constraints: &[Constraint]) -> String {
    if constraints.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = constraints
        .iter()
        .filter(|c| !c.is_let())
        .map(ToString::to_string)
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    format!(" where {}", parts.join(", "))
}

/// The `let` clauses among `constraints`, each as ` let …`, or `""`.
pub fn display_lets(constraints: &[Constraint]) -> String {
    constraints
        .iter()
        .filter(|c| c.is_let())
        .map(|c| format!(" {c}"))
        .collect()
}

impl Constraint {
    /// `true` for a `let`, which derives a value rather than testing one.
    pub fn is_let(&self) -> bool {
        matches!(self.test, Test::Let { .. })
    }
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
