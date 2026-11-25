//! `where` constraints on colab's own matches: the same tests as
//! `pattern/check.rs` (the ast-grep engine's), read from
//! `colab_syntax::pattern::Match` and `colab_syntax::Node`.
//!
//! A match inside a macro's contents is judged as one in the file would be
//! ([`Frame`]): what encloses it is the contents' own nodes and then the
//! macro call and its ancestors in the file, never the wrapper; its
//! siblings are the contents' own; a language server is asked about the
//! file at the position the contents came from.

use colab_core::{Constraint, Relation, Stop, Test, Var};
use colab_syntax::pattern::{Binding, Match, MetaVar, Pattern, PatternNode};
use colab_syntax::{Grammar, Node};

use crate::pattern::PatternLanguage;

/// A nested pattern (`inside`'s, `has`'s, …) in every form it compiles
/// to; its metavariables are its own.
pub struct Nested(Vec<Pattern>);

impl Nested {
    fn compile(language: &PatternLanguage, keyword: &str, text: &str) -> Result<Self, String> {
        let forms = super::compile_forms(language, text)
            .map_err(|e| format!("`{keyword} '{text}'`: {e}"))?;
        Ok(Self(forms.forms.into_iter().map(|f| f.pattern).collect()))
    }

    pub fn matches(&self, node: Node<'_>) -> bool {
        self.0.iter().any(|p| p.matches(node).is_some())
    }
}

/// Whether an enclosing node outside a macro's contents matches.
pub type Enclosing<'f> = Box<dyn Fn(&Nested) -> bool + 'f>;

/// Where a macro's contents, parsed in the wrapper, sit in the file.
pub struct Frame<'f> {
    /// The contents' span in the wrapped text.
    pub contents: std::ops::Range<usize>,
    /// The macro call and its ancestors in the file (nearest first), then
    /// an enclosing frame's.
    pub outside: &'f [Enclosing<'f>],
    /// The file's text, which a language server is asked about.
    pub file: &'f str,
    /// A position in the wrapped text, as one in `file`.
    pub map: Box<dyn Fn(usize) -> usize + 'f>,
}

impl Frame<'_> {
    /// Whether `node` is the contents' own, not the wrapper's.
    pub fn holds(&self, node: Node<'_>) -> bool {
        node.start() >= self.contents.start && node.end() <= self.contents.end
    }

    fn to_file(&self, span: std::ops::Range<usize>) -> std::ops::Range<usize> {
        (self.map)(span.start)..(self.map)(span.end)
    }
}

enum StopAt {
    Anywhere,
    Directly,
    Until(Nested),
}

enum Kind {
    Matches {
        var: Var,
        regex: regex::Regex,
    },
    Is {
        name: String,
        kind: String,
    },
    Count {
        name: String,
        min: usize,
        max: Option<usize>,
    },
    Inside(Nested),
    Has(Nested),
    Related {
        relation: Relation,
        pattern: Nested,
        stop: StopAt,
    },
    Any(Vec<Check>),
    All(Vec<Check>),
    Nth(i64),
    Field(String),
    Top,
    In {
        var: Var,
        set: String,
        field: String,
    },
    Semantic {
        var: Var,
        query: colab_core::semantic::Query,
    },
}

struct Check {
    negated: bool,
    kind: Kind,
}

/// A pattern's compiled `where` constraints (`let`s aside: they derive
/// text for a rewrite and narrow nothing).
#[derive(Default)]
pub struct Checks(Vec<Check>);

/// The metavariables a pattern binds, as `where` names them.
pub fn bound(pattern: &Pattern) -> Vec<Var> {
    let mut out: Vec<Var> = Vec::new();
    let mut stack = vec![pattern.root()];
    while let Some(node) = stack.pop() {
        match node {
            PatternNode::Var(MetaVar::One { name: Some(n), .. }) => out.push(Var {
                name: n.clone(),
                multi: false,
            }),
            PatternNode::Var(MetaVar::Many { name: Some(n) }) => out.push(Var {
                name: n.clone(),
                multi: true,
            }),
            PatternNode::Node { children, .. } => stack.extend(children.iter().rev()),
            _ => {}
        }
    }
    out.dedup_by(|a, b| a.name == b.name);
    out
}

impl Checks {
    /// `constraints` compiled for a pattern binding `bound`.
    pub fn compile(
        language: &PatternLanguage,
        bound: &[Var],
        constraints: &[Constraint],
    ) -> Result<Self, String> {
        let grammar = super::grammar_of(&language.grammar);
        let checks = constraints
            .iter()
            .filter(|c| !matches!(c.test, Test::Let { .. }))
            .map(|c| Check::compile(language, &grammar, bound, c))
            .collect::<Result<_, _>>()?;
        Ok(Self(checks))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The matches in `found` that every check holds for: the cheap checks
    /// first, then each question for the language server, asked about all
    /// the survivors at once.
    pub fn retain(&self, found: &mut Vec<Match<'_>>, source: &str, frame: Option<&Frame<'_>>) {
        let (asked, cheap): (Vec<&Check>, Vec<&Check>) = self
            .0
            .iter()
            .partition(|c| matches!(c.kind, Kind::Semantic { .. }));
        found.retain(|m| cheap.iter().all(|c| c.holds(m, source, frame)));
        for check in asked {
            if let Kind::Semantic { var, query } = &check.kind {
                let spans: Vec<_> = found
                    .iter()
                    .filter_map(|m| span_of(var, m))
                    .map(|s| frame.map_or(s.clone(), |f| f.to_file(s)))
                    .collect();
                colab_core::semantic::prefetch(frame.map_or(source, |f| f.file), &spans, query);
            }
            found.retain(|m| check.holds(m, source, frame));
        }
    }
}

fn require_bound(bound: &[Var], var: &Var) -> Result<(), String> {
    crate::pattern::require_bound(bound, var)
}

impl Check {
    fn compile(
        language: &PatternLanguage,
        grammar: &Grammar,
        bound: &[Var],
        constraint: &Constraint,
    ) -> Result<Self, String> {
        let nested = |keyword: &str, text: &str| Nested::compile(language, keyword, text);
        let kind = match &constraint.test {
            Test::Matches { var, regex } => {
                require_bound(bound, var)?;
                let regex = regex::Regex::new(&regex.replace(r"\/", "/"))
                    .map_err(|e| format!("`{var} matches /{regex}/`: {e}"))?;
                Kind::Matches {
                    var: var.clone(),
                    regex,
                }
            }
            Test::Is { var, kind } => {
                require_bound(bound, var)?;
                if var.multi {
                    return Err(format!(
                        "`{var} is {kind}`: `is` checks one node; bind it as `${}`",
                        var.name
                    ));
                }
                let named = grammar
                    .kinds_named(kind)
                    .into_iter()
                    .any(|k| grammar.kind_is_named(k));
                if !named {
                    return Err(format!(
                        "`{var} is {kind}`: the grammar has no node kind `{kind}`"
                    ));
                }
                Kind::Is {
                    name: var.name.clone(),
                    kind: kind.clone(),
                }
            }
            Test::Count { var, min, max } => {
                require_bound(bound, var)?;
                if !var.multi {
                    return Err(format!(
                        "`{var} count`: `count` measures a sequence; bind it as `$$${}`",
                        var.name
                    ));
                }
                if max.is_some_and(|max| max < *min) {
                    return Err(format!("`{constraint}`: the range is empty"));
                }
                Kind::Count {
                    name: var.name.clone(),
                    min: *min,
                    max: *max,
                }
            }
            Test::Inside(text) => Kind::Inside(nested("inside", text)?),
            Test::Has(text) => Kind::Has(nested("has", text)?),
            Test::Related {
                relation,
                pattern,
                stop,
            } => Kind::Related {
                relation: *relation,
                pattern: nested(relation.keyword(), pattern)?,
                stop: match stop {
                    Stop::Anywhere => StopAt::Anywhere,
                    Stop::Directly => StopAt::Directly,
                    Stop::Until(until) => StopAt::Until(nested("until", until)?),
                },
            },
            Test::Any(all) | Test::All(all) => {
                if all.is_empty() {
                    return Err(format!("`{constraint}` needs at least one constraint"));
                }
                let checks = all
                    .iter()
                    .map(|c| Self::compile(language, grammar, bound, c))
                    .collect::<Result<Vec<_>, _>>()?;
                if matches!(constraint.test, Test::Any(_)) {
                    Kind::Any(checks)
                } else {
                    Kind::All(checks)
                }
            }
            Test::Nth(0) => {
                return Err(format!(
                    "`{constraint}`: positions count from 1 (`nth 1` is the first, `nth -1` the last)"
                ));
            }
            Test::Nth(n) => Kind::Nth(*n),
            Test::In { var, set, field } => {
                require_bound(bound, var)?;
                Kind::In {
                    var: var.clone(),
                    set: set.clone(),
                    field: field
                        .as_ref()
                        .map_or_else(|| var.name.clone(), |f| f.name.clone()),
                }
            }
            Test::Semantic { var, query } => {
                require_bound(bound, var)?;
                Kind::Semantic {
                    var: var.clone(),
                    query: query.clone(),
                }
            }
            Test::Field(name) => {
                if grammar.field(name).is_none() {
                    return Err(format!("`{constraint}`: the grammar has no field `{name}`"));
                }
                Kind::Field(name.clone())
            }
            Test::Top => Kind::Top,
            Test::Let { .. } => unreachable!("`let`s are filtered out"),
        };
        Ok(Self {
            negated: constraint.negated,
            kind,
        })
    }

    fn holds(&self, m: &Match<'_>, source: &str, frame: Option<&Frame<'_>>) -> bool {
        let node = m.node;
        let result = match &self.kind {
            Kind::Matches { var, regex } => regex.is_match(&text_of(var, m)),
            Kind::Is { name, kind } => {
                matches!(m.get(name), Some(Binding::One(n)) if n.kind_name() == kind)
            }
            Kind::Count { name, min, max } => {
                // Separators are anonymous: count what a reader would.
                let n = match m.get(name) {
                    Some(Binding::Many(nodes)) => nodes.iter().filter(|n| n.is_named()).count(),
                    _ => 0,
                };
                n >= *min && max.is_none_or(|max| n <= max)
            }
            Kind::Inside(nested) => ancestors(node, frame).iter().any(|(a, _)| a(nested)),
            Kind::Has(nested) => node.descendants().skip(1).any(|n| nested.matches(n)),
            Kind::Related {
                relation,
                pattern,
                stop,
            } => related(node, *relation, pattern, stop, frame),
            Kind::Any(checks) => checks.iter().any(|c| c.holds(m, source, frame)),
            Kind::All(checks) => checks.iter().all(|c| c.holds(m, source, frame)),
            Kind::Nth(n) => is_nth(node, *n),
            Kind::Field(name) => same_span(node)
                .iter()
                .any(|c| c.field_name() == Some(name.as_str())),
            // A macro's contents are inside the call, never the file's own items.
            Kind::Top => {
                frame.is_none()
                    && same_span(node)
                        .iter()
                        .any(|n| n.parent().is_some_and(|p| p.parent().is_none()))
            }
            Kind::In { var, set, field } => {
                colab_core::facts::contains(set, field, &text_of(var, m))
            }
            Kind::Semantic { var, query } => span_of(var, m).is_some_and(|span| match frame {
                Some(f) => colab_core::semantic::ask(f.file, f.to_file(span), query),
                None => colab_core::semantic::ask(source, span, query),
            }),
        };
        result != self.negated
    }
}

fn text_of(var: &Var, m: &Match<'_>) -> String {
    m.get(&var.name)
        .map_or_else(String::new, |b| b.text().to_string())
}

/// The code a semantic check asks about; `None` for an empty sequence.
fn span_of(var: &Var, m: &Match<'_>) -> Option<std::ops::Range<usize>> {
    m.get(&var.name).and_then(Binding::range)
}

/// `node`, or its outermost ancestor spanning exactly the same text: a
/// matched call is also its expression statement.
fn widest(node: Node<'_>) -> Node<'_> {
    let mut n = node;
    while let Some(parent) = n.parent() {
        if parent.range() != n.range() {
            break;
        }
        n = parent;
    }
    n
}

/// `node` and each ancestor spanning the same text, innermost first.
fn same_span(node: Node<'_>) -> Vec<Node<'_>> {
    let mut out = vec![node];
    while let Some(parent) = out.last().and_then(Node::parent) {
        if parent.range() != node.range() {
            break;
        }
        out.push(parent);
    }
    out
}

/// `node`'s ancestors, nearest first, each with whether it is larger;
/// in a macro's contents, the contents' own and then those outside.
fn ancestors<'a, 'f: 'a>(
    node: Node<'a>,
    frame: Option<&'a Frame<'f>>,
) -> Vec<(Enclosing<'a>, bool)> {
    let range = node.range();
    let mut out: Vec<(Enclosing<'a>, bool)> = std::iter::successors(node.parent(), Node::parent)
        .filter(|a| frame.is_none_or(|f| f.holds(*a)))
        .map(|a| -> (Enclosing<'a>, bool) {
            let larger = a.range() != range;
            (Box::new(move |nested: &Nested| nested.matches(a)), larger)
        })
        .collect();
    if let Some(frame) = frame {
        out.extend(frame.outside.iter().map(|outer| -> (Enclosing<'a>, bool) {
            (Box::new(move |nested: &Nested| outer(nested)), true)
        }));
    }
    out
}

fn related(
    node: Node<'_>,
    relation: Relation,
    pattern: &Nested,
    stop: &StopAt,
    frame: Option<&Frame<'_>>,
) -> bool {
    let real = |n: &Node<'_>| n.is_named() && !n.is_extra() && frame.is_none_or(|f| f.holds(*n));
    let siblings = |next: bool| {
        let base = widest(node);
        let step = if next {
            Node::next_sibling
        } else {
            Node::prev_sibling
        };
        std::iter::successors(step(&base), step)
            .filter(real)
            .collect::<Vec<_>>()
    };
    let search = |candidates: Vec<Node<'_>>| {
        for c in candidates {
            if pattern.matches(c) {
                return true;
            }
            match stop {
                StopAt::Anywhere => {}
                StopAt::Directly => return false,
                StopAt::Until(until) if until.matches(c) => return false,
                StopAt::Until(_) => {}
            }
        }
        false
    };
    match relation {
        Relation::Inside => {
            // `directly`: up to the first enclosing node larger than the match.
            for (candidate, larger) in ancestors(node, frame) {
                if candidate(pattern) {
                    return true;
                }
                match stop {
                    StopAt::Anywhere => {}
                    StopAt::Directly if larger => return false,
                    StopAt::Directly => {}
                    StopAt::Until(until) if candidate(until) => return false,
                    StopAt::Until(_) => {}
                }
            }
            false
        }
        Relation::Has => match stop {
            StopAt::Directly => node.children().any(|c| pattern.matches(c)),
            StopAt::Anywhere => node.descendants().skip(1).any(|n| pattern.matches(n)),
            StopAt::Until(until) => {
                let mut stack: Vec<Node<'_>> = node.children().collect();
                while let Some(n) = stack.pop() {
                    if pattern.matches(n) {
                        return true;
                    }
                    if !until.matches(n) {
                        stack.extend(n.children());
                    }
                }
                false
            }
        },
        Relation::Follows => search(siblings(false)),
        Relation::Precedes => search(siblings(true)),
    }
}

/// Whether `node` is its parent's `n`th named child (negative: from the end).
fn is_nth(node: Node<'_>, n: i64) -> bool {
    let base = widest(node);
    let Some(parent) = base.parent() else {
        return false;
    };
    let siblings: Vec<_> = parent
        .children()
        .filter(|c| c.is_named() && !c.is_extra())
        .map(|c| c.range())
        .collect();
    let Some(at) = siblings.iter().position(|r| *r == base.range()) else {
        return false;
    };
    let (Ok(len), Ok(at)) = (i64::try_from(siblings.len()), i64::try_from(at)) else {
        return false;
    };
    if n > 0 { at + 1 == n } else { len - at == -n }
}
