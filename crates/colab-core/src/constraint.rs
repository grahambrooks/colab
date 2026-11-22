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
    /// `inside` / `has` with a stop, and `follows` / `precedes`: some
    /// node in that relation to the match matches `pattern`, looking no
    /// further than `stop` allows.
    Related {
        relation: Relation,
        pattern: String,
        stop: Stop,
    },
    /// `any(c, …)`: at least one of the constraints holds.
    Any(Vec<Constraint>),
    /// `all(c, …)`: every one holds (a comma does the same at the top
    /// level; `all` groups them under `not` or `any`).
    All(Vec<Constraint>),
    /// `$X in <set>` / `$X in <set>.$F`: the text bound to `$X` was bound
    /// to `field` by the `scan … as <set>` (see [`crate::facts`]). The
    /// compiler fills in `field` when the script leaves it out.
    In {
        var: Var,
        set: String,
        field: Option<Var>,
    },
    /// `$X type "io.Reader"` / `$F resolves "pkg.Func"`: answered by the
    /// language server, through [`crate::semantic`].
    Semantic {
        var: Var,
        query: crate::semantic::Query,
    },
    /// `nth n`: the match is its parent's `n`th named child, from 1; a
    /// negative `n` counts from the end (`nth -1` is the last).
    Nth(i64),
    /// `field <name>`: the match is its parent's child in that field
    /// (`function` of a call, `body` of a loop, …).
    Field(String),
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

/// The pattern text a constraint holds for a reference to a named pattern
/// (`inside helper`, after `pattern helper = '…'`), until the compiler
/// replaces it with the definition. A control character marks it, so no
/// pattern written in quotes is mistaken for one.
pub fn named_pattern(name: &str) -> String {
    format!("{NAMED}{name}")
}

/// The name, when `pattern` is an unresolved [`named_pattern`] reference.
pub fn pattern_name(pattern: &str) -> Option<&str> {
    pattern.strip_prefix(NAMED)
}

const NAMED: char = '\u{1}';

/// A pattern as written in a constraint: `'…'`, or a bare name.
struct Shown<'a>(&'a str);

impl fmt::Display for Shown<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match pattern_name(self.0) {
            Some(name) => f.write_str(name),
            None => write!(f, "'{}'", self.0),
        }
    }
}

impl Test {
    /// `true` when this test, or one grouped in it, needs a language
    /// server ([`Test::Semantic`]).
    pub fn is_semantic(&self) -> bool {
        match self {
            Test::Semantic { .. } => true,
            Test::Any(all) | Test::All(all) => all.iter().any(|c| c.test.is_semantic()),
            _ => false,
        }
    }

    /// Every pattern text this test holds, nested groups included, for
    /// the compiler to resolve named references in place.
    pub fn patterns_mut(&mut self) -> Vec<&mut String> {
        match self {
            Test::Inside(p) | Test::Has(p) => vec![p],
            Test::Related { pattern, stop, .. } => match stop {
                Stop::Until(until) => vec![pattern, until],
                _ => vec![pattern],
            },
            Test::Any(all) | Test::All(all) => {
                all.iter_mut().flat_map(|c| c.test.patterns_mut()).collect()
            }
            _ => Vec::new(),
        }
    }
}

/// Where a [`Test::Related`] pattern is looked for, relative to the match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// An enclosing node.
    Inside,
    /// A node within the match.
    Has,
    /// A sibling before the match.
    Follows,
    /// A sibling after the match.
    Precedes,
}

impl Relation {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Inside => "inside",
            Self::Has => "has",
            Self::Follows => "follows",
            Self::Precedes => "precedes",
        }
    }
}

/// How far a [`Test::Related`] search goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// As far as the tree goes: every ancestor, descendant or sibling.
    Anywhere,
    /// `directly`: the nearest one only (the enclosing node, a child, the
    /// adjacent sibling).
    Directly,
    /// `until '<pattern>'`: up to, not past, a node matching it.
    Until(String),
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
    /// `rewrite(<name>, …)`, after `rewriter <name> = '<pattern>' ->
    /// '<template>'`: each match of a rewriter's pattern inside the
    /// binding, rewritten with its template, the text between them kept.
    /// The compiler fills in each rewriter; it is a `let`'s first step.
    Rewrite(Vec<Rewriter>),
}

/// A named `rewriter`, as a [`Transform::Rewrite`] step applies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewriter {
    pub name: String,
    pub pattern: String,
    pub template: String,
}

/// An argument to a transform, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransformArg {
    Text(String),
    Regex(String),
    Int(i64),
    /// A bare name: the rewriter `rewrite(<name>)` applies.
    Name(String),
}

impl fmt::Display for TransformArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(t) => write!(f, "\"{t}\""),
            Self::Regex(r) => write!(f, "/{r}/"),
            Self::Int(n) => write!(f, "{n}"),
            Self::Name(name) => f.write_str(name),
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
    "rewrite",
];

impl Transform {
    /// The step `name(args)`, or why it is not one.
    pub fn parse(name: &str, args: Vec<TransformArg>) -> Result<Self, String> {
        use TransformArg::{Int, Name, Regex, Text};
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
            // The compiler follows each name with its rewriter's pattern
            // and template.
            "rewrite" => {
                let wrong = || {
                    format!(
                        "`{}`: `rewrite` takes the names of rewriters, `rewrite(<name>, …)`",
                        shown()
                    )
                };
                if args.is_empty() || !args.len().is_multiple_of(3) {
                    return Err(wrong());
                }
                args.chunks(3)
                    .map(|chunk| match chunk {
                        [Name(name), Text(pattern), Text(template)] => Ok(Rewriter {
                            name: name.clone(),
                            pattern: pattern.clone(),
                            template: template.clone(),
                        }),
                        _ => Err(wrong()),
                    })
                    .collect::<Result<_, _>>()
                    .map(Self::Rewrite)
            }
            _ => {
                let hint = crate::suggest::closest(name, TRANSFORMS.iter().copied())
                    .map_or_default(|t| format!(" (did you mean `{t}`?)"));
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
            Self::Rewrite(rewriters) => {
                let names: Vec<&str> = rewriters.iter().map(|r| r.name.as_str()).collect();
                write!(f, "rewrite({})", names.join(", "))
            }
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
            Test::Inside(pattern) => write!(f, "inside {}", Shown(pattern)),
            Test::Has(pattern) => write!(f, "has {}", Shown(pattern)),
            Test::Related {
                relation,
                pattern,
                stop,
            } => {
                write!(f, "{} {}", relation.keyword(), Shown(pattern))?;
                match stop {
                    Stop::Anywhere => Ok(()),
                    Stop::Directly => f.write_str(" directly"),
                    Stop::Until(until) => write!(f, " until {}", Shown(until)),
                }
            }
            Test::Any(all) | Test::All(all) => {
                let name = if matches!(self.test, Test::Any(_)) {
                    "any"
                } else {
                    "all"
                };
                let parts: Vec<String> = all.iter().map(ToString::to_string).collect();
                write!(f, "{name}({})", parts.join(", "))
            }
            Test::Nth(n) => write!(f, "nth {n}"),
            Test::Semantic { var, query } => write!(f, "{var} {query}"),
            Test::In { var, set, field } => match field {
                Some(field) => write!(f, "{var} in {set}.{field}"),
                None => write!(f, "{var} in {set}"),
            },
            Test::Field(name) => write!(f, "field {name}"),
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
