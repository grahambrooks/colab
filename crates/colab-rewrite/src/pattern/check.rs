//! `where` constraints and `let` derivations, evaluated as a filter on the
//! match list: relations (`inside`, `has`, siblings), positions, fields,
//! regexes, nested patterns, and the semantic and scan-fact tests.

use super::*;

/// One compiled `let`: `var` is `from`'s text through `steps`.
#[derive(Debug)]
pub(super) struct Derived {
    pub(super) var: String,
    pub(super) from: String,
    /// A first `rewrite(<name>, …)` step: each rewriter's pattern,
    /// compiled for this language, and its template (empty without one).
    pub(super) rewriters: Vec<(StructuralPattern, String)>,
    pub(super) steps: Vec<crate::derive::Step>,
}

impl Derived {
    /// The `let`s among `constraints`, in order. Each derives from a
    /// metavariable the pattern binds or an earlier `let`, and names a
    /// new `$VAR`, written with one `$`: it holds text, not nodes.
    pub(super) fn compile_all(
        language: &PatternLanguage,
        bound: &[Var],
        constraints: &[Constraint],
    ) -> Result<Vec<Self>, String> {
        let mut out: Vec<Self> = Vec::new();
        for c in constraints {
            let Test::Let { var, from, steps } = &c.test else {
                continue;
            };
            if var.multi {
                return Err(format!(
                    "`{c}`: a `let` holds text, so it is named with one `$`: `${}`",
                    var.name
                ));
            }
            if let Some(b) = bound.iter().find(|b| b.name == var.name) {
                return Err(format!(
                    "`{c}`: the pattern already binds `{b}`; name the derived value something else"
                ));
            }
            if out.iter().any(|d| d.var == var.name) {
                return Err(format!("`{c}`: `{var}` is already a `let`"));
            }
            let earlier = !from.multi && out.iter().any(|d| d.var == from.name);
            if !earlier {
                require_bound(bound, from).map_err(|e| format!("`{c}`: {e}"))?;
            }
            let (rewriters, rest) = match steps.split_first() {
                Some((colab_core::Transform::Rewrite(list), rest)) => {
                    if earlier {
                        return Err(format!(
                            "`{c}`: a rewriter works on the code a metavariable binds, so `rewrite` takes `{from}` from the pattern, not from a `let`"
                        ));
                    }
                    let compiled = list
                        .iter()
                        .map(|r| {
                            unbound_in_template(&r.pattern, &r.template, &[])
                                .and_then(|()| StructuralPattern::compile(language, &r.pattern))
                                .map(|sub| (sub, r.template.clone()))
                                .map_err(|e| format!("`rewriter {}`: {e}", r.name))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    (compiled, rest)
                }
                _ => (Vec::new(), steps.as_slice()),
            };
            if let Some(late) = rest
                .iter()
                .find(|s| matches!(s, colab_core::Transform::Rewrite(_)))
            {
                return Err(format!(
                    "`{c}`: `{late}` must be the first step: a rewriter works on the code, not on text another step made"
                ));
            }
            out.push(Self {
                var: var.name.clone(),
                from: from.name.clone(),
                rewriters,
                steps: rest
                    .iter()
                    .map(crate::derive::Step::compile)
                    .collect::<Result<_, _>>()?,
            });
        }
        Ok(out)
    }

    pub(super) fn apply(&self, text: &str) -> String {
        self.steps
            .iter()
            .fold(text.to_string(), |text, step| step.apply(&text))
    }
}

/// One compiled `where` constraint.
pub(super) struct Check {
    pub(super) negated: bool,
    pub(super) kind: CheckKind,
}

pub(super) enum CheckKind {
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
    Inside(Forms),
    Has(Forms),
    Related {
        relation: Relation,
        pattern: Forms,
        stop: StopAt,
    },
    Any(Vec<Check>),
    All(Vec<Check>),
    Nth(i64),
    Field(String),
    /// A direct child of the file's root ([`is_top`]).
    Top,
    /// Read from the scan phases' facts, through `colab_core::facts`.
    In {
        var: Var,
        set: String,
        field: String,
    },
    /// Asked of the language server, through `colab_core::semantic`.
    Semantic {
        var: Var,
        query: colab_core::semantic::Query,
    },
}

/// A compiled [`Stop`].
pub(super) enum StopAt {
    Anywhere,
    Directly,
    Until(Forms),
}

/// A nested pattern as every context compiled it: `retries: $N` is a
/// label on its own and a property in an object, and a nested pattern
/// matches either.
pub(super) struct Forms(Vec<Pattern>);

impl Forms {
    pub(super) fn match_node(&self, node: &TsNode<'_>) -> bool {
        self.0.iter().any(|p| node.matches(p))
    }
}

pub(super) type TsNode<'t> = ast_grep_core::Node<'t, StrDoc<TsLang>>;

/// `node`, or the outermost ancestor spanning exactly the same text: a
/// matched call is also its expression statement, so its siblings are the
/// statements around it.
pub(super) fn widest<'t>(node: &TsNode<'t>) -> TsNode<'t> {
    let mut n = node.clone();
    while let Some(parent) = n.parent() {
        if parent.range() != n.range() {
            break;
        }
        n = parent;
    }
    n
}

/// `node` and each ancestor spanning the same text, innermost first.
pub(super) fn same_span<'t>(node: &TsNode<'t>) -> Vec<TsNode<'t>> {
    let mut out = vec![node.clone()];
    while let Some(parent) = out.last().and_then(ast_grep_core::Node::parent) {
        if parent.range() != node.range() {
            break;
        }
        out.push(parent);
    }
    out
}

/// `node`'s ancestors, nearest first, each with whether it is larger than
/// `node` (`directly` stops at the first that is). In a macro's contents
/// (`frame`), only the ancestors inside the contents count, and then the
/// macro call and its ancestors in the file take the wrapper's place.
pub(super) fn ancestors<'n, 'a: 'n>(
    node: &TsNode<'a>,
    frame: Option<&'n Frame<'n>>,
) -> Vec<(Enclosing<'n>, bool)> {
    let range = node.range();
    let mut out: Vec<(Enclosing<'n>, bool)> = node
        .ancestors()
        .filter(|a| frame.is_none_or(|f| f.holds(a)))
        .map(|a| -> (Enclosing<'n>, bool) {
            let larger = a.range() != range;
            (Box::new(move |forms| forms.match_node(&a)), larger)
        })
        .collect();
    if let Some(frame) = frame {
        out.extend(
            frame.outside.iter().map(|outer| -> (Enclosing<'n>, bool) {
                (Box::new(move |forms| outer(forms)), true)
            }),
        );
    }
    out
}

/// Whether some node related to `node` matches `pattern`, searched
/// nearest first and no further than `stop`.
pub(super) fn related(
    node: &TsNode<'_>,
    relation: Relation,
    pattern: &Forms,
    stop: &StopAt,
    frame: Option<&Frame<'_>>,
) -> bool {
    // Candidates nearest first, and whether each is the last `directly`
    // allows.
    let search = |candidates: &mut dyn Iterator<Item = TsNode<'_>>,
                  last: &dyn Fn(&TsNode<'_>) -> bool| {
        for c in candidates {
            if pattern.match_node(&c) {
                return true;
            }
            match stop {
                StopAt::Anywhere => {}
                StopAt::Directly if last(&c) => return false,
                StopAt::Directly => {}
                StopAt::Until(until) if until.match_node(&c) => return false,
                StopAt::Until(_) => {}
            }
        }
        false
    };
    // A sibling outside a macro's contents is the wrapper's, not the file's.
    let real = |n: &TsNode<'_>| n.is_named() && !n.is_extra() && frame.is_none_or(|f| f.holds(n));
    match relation {
        // `directly`: up to the first enclosing node that is larger than
        // the match (wrappers over the same text do not count).
        Relation::Inside => {
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
            StopAt::Directly => node.children().any(|c| pattern.match_node(&c)),
            StopAt::Anywhere => node.dfs().skip(1).any(|n| pattern.match_node(&n)),
            // Depth first, not descending into a node matching `until`.
            StopAt::Until(until) => {
                let mut stack: Vec<TsNode<'_>> = node.children().collect();
                while let Some(n) = stack.pop() {
                    if pattern.match_node(&n) {
                        return true;
                    }
                    if !until.match_node(&n) {
                        stack.extend(n.children());
                    }
                }
                false
            }
        },
        Relation::Follows => {
            let base = widest(node);
            search(&mut base.prev_all().filter(real), &|_: &TsNode<'_>| true)
        }
        Relation::Precedes => {
            let base = widest(node);
            search(&mut base.next_all().filter(real), &|_: &TsNode<'_>| true)
        }
    }
}

/// Whether `node` is its parent's `n`th named child (negative: from the end).
pub(super) fn is_nth(node: &TsNode<'_>, n: i64) -> bool {
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
    let len = siblings.len() as i64;
    let at = at as i64;
    if n > 0 { at + 1 == n } else { len - at == -n }
}

/// Whether `node` (or a node spanning the same text) is a direct child of
/// the tree's root: one of the file's own items or statements. A match
/// that spans the whole file is the root, which is no item of itself.
pub(super) fn is_top(node: &TsNode<'_>) -> bool {
    same_span(node)
        .iter()
        .any(|n| n.parent().is_some_and(|parent| parent.parent().is_none()))
}

/// Whether `node` (or a node spanning the same text) is its parent's
/// child in field `name`.
pub(super) fn in_field(node: &TsNode<'_>, name: &str) -> bool {
    same_span(node).iter().any(|c| {
        c.parent().is_some_and(|parent| {
            parent
                .field_children(name)
                .any(|f| f.range() == c.range() && f.kind() == c.kind())
        })
    })
}

impl Check {
    pub(super) fn compile(
        language: &PatternLanguage,
        bound: &[Var],
        constraint: &Constraint,
    ) -> Result<Self, String> {
        let kind = match &constraint.test {
            Test::Matches { var, regex } => {
                require_bound(bound, var)?;
                // `/` ends the DSL's regex literal, so it is written `\/`.
                let regex = regex::Regex::new(&regex.replace(r"\/", "/"))
                    .map_err(|e| format!("`{var} matches /{regex}/`: {e}"))?;
                CheckKind::Matches {
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
                if language.grammar.id_for_node_kind(kind, true) == 0 {
                    let kinds = named_kinds(&language.grammar);
                    let hint = colab_core::suggest::closest(kind, kinds.iter().copied())
                        .map_or_default(|k| format!(" (did you mean `{k}`?)"));
                    return Err(format!(
                        "`{var} is {kind}`: the grammar has no node kind `{kind}`{hint}; `colab ast` shows the kinds in a snippet"
                    ));
                }
                CheckKind::Is {
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
                CheckKind::Count {
                    name: var.name.clone(),
                    min: *min,
                    max: *max,
                }
            }
            Test::Inside(text) => CheckKind::Inside(nested(language, "inside", text)?),
            Test::Has(text) => CheckKind::Has(nested(language, "has", text)?),
            Test::Related {
                relation,
                pattern,
                stop,
            } => CheckKind::Related {
                relation: *relation,
                pattern: nested(language, relation.keyword(), pattern)?,
                stop: match stop {
                    Stop::Anywhere => StopAt::Anywhere,
                    Stop::Directly => StopAt::Directly,
                    Stop::Until(until) => StopAt::Until(nested(language, "until", until)?),
                },
            },
            Test::Any(all) | Test::All(all) => {
                if all.is_empty() {
                    return Err(format!("`{constraint}` needs at least one constraint"));
                }
                let checks = all
                    .iter()
                    .map(|c| Self::compile(language, bound, c))
                    .collect::<Result<Vec<_>, _>>()?;
                if matches!(constraint.test, Test::Any(_)) {
                    CheckKind::Any(checks)
                } else {
                    CheckKind::All(checks)
                }
            }
            Test::Nth(0) => {
                return Err(format!(
                    "`{constraint}`: positions count from 1 (`nth 1` is the first, `nth -1` the last)"
                ));
            }
            Test::Nth(n) => CheckKind::Nth(*n),
            Test::In { var, set, field } => {
                require_bound(bound, var)?;
                CheckKind::In {
                    var: var.clone(),
                    set: set.clone(),
                    field: field
                        .as_ref()
                        .map_or_else(|| var.name.clone(), |f| f.name.clone()),
                }
            }
            Test::Semantic { var, query } => {
                require_bound(bound, var)?;
                CheckKind::Semantic {
                    var: var.clone(),
                    query: query.clone(),
                }
            }
            Test::Field(name) => {
                if language.grammar.field_id_for_name(name).is_none() {
                    let fields: Vec<&str> = (1..=u16::try_from(language.grammar.field_count())
                        .unwrap_or(u16::MAX))
                        .filter_map(|id| language.grammar.field_name_for_id(id))
                        .collect();
                    let hint = colab_core::suggest::closest(name, fields.iter().copied())
                        .map_or_default(|f| format!(" (did you mean `{f}`?)"));
                    return Err(format!(
                        "`{constraint}`: the grammar has no field `{name}`{hint}; `colab ast` shows the fields in a snippet"
                    ));
                }
                CheckKind::Field(name.clone())
            }
            Test::Top => CheckKind::Top,
            Test::Let { .. } => unreachable!("`let` is compiled by `Derived`"),
        };
        Ok(Self {
            negated: constraint.negated,
            kind,
        })
    }

    /// Whether this is a top-level question for the language server.
    pub(super) fn is_semantic(&self) -> bool {
        matches!(self.kind, CheckKind::Semantic { .. })
    }

    /// Ask a semantic check's question about every match in `found` at
    /// once ([`colab_core::semantic::prefetch`]); a no-op for any other.
    /// In a macro's contents (`frame`) it is asked of the file, at the
    /// position the contents came from.
    pub(super) fn prefetch(
        &self,
        found: &[NodeMatch<'_, StrDoc<TsLang>>],
        source: &str,
        frame: Option<&Frame<'_>>,
    ) {
        if let CheckKind::Semantic { var, query } = &self.kind {
            let spans: Vec<_> = found
                .iter()
                .filter_map(|m| semantic_span(var, m))
                .map(|span| frame.map_or(span.clone(), |f| f.to_file(span)))
                .collect();
            let file = frame.map_or(source, |f| f.file);
            colab_core::semantic::prefetch(file, &spans, query);
        }
    }

    /// Whether match `m` in `source` satisfies this constraint. In a
    /// macro's contents (`frame`), what surrounds the match is read from
    /// the contents and then the file, never the wrapper (see [`Frame`]).
    pub(super) fn holds(
        &self,
        m: &NodeMatch<'_, StrDoc<TsLang>>,
        source: &str,
        frame: Option<&Frame<'_>>,
    ) -> bool {
        let env = m.get_env();
        let result = match &self.kind {
            CheckKind::Matches { var, regex } => {
                let text = if var.multi {
                    let nodes = env.get_multiple_matches(&var.name);
                    match (nodes.first(), nodes.last()) {
                        (Some(first), Some(last)) => {
                            source[first.range().start..last.range().end].to_string()
                        }
                        _ => String::new(),
                    }
                } else {
                    env.get_match(&var.name)
                        .map_or_default(|n| n.text().into_owned())
                };
                regex.is_match(&text)
            }
            CheckKind::Is { name, kind } => env.get_match(name).is_some_and(|n| n.kind() == *kind),
            CheckKind::Count { name, min, max } => {
                // Separators (`,`) are unnamed nodes in the sequence; count
                // what a reader would count.
                let n = env
                    .get_multiple_matches(name)
                    .iter()
                    .filter(|n| n.is_named())
                    .count();
                n >= *min && max.is_none_or(|max| n <= max)
            }
            CheckKind::Inside(forms) => {
                ancestors(m.get_node(), frame).iter().any(|(a, _)| a(forms))
            }
            CheckKind::Has(forms) => m.get_node().dfs().skip(1).any(|n| forms.match_node(&n)),
            CheckKind::Related {
                relation,
                pattern,
                stop,
            } => related(m.get_node(), *relation, pattern, stop, frame),
            CheckKind::Any(checks) => checks.iter().any(|c| c.holds(m, source, frame)),
            CheckKind::All(checks) => checks.iter().all(|c| c.holds(m, source, frame)),
            // Among the contents' own nodes: a macro's top-level arguments
            // are the wrapper call's, so `nth` counts them and they are in
            // no `field`, as the contents of a token tree are.
            CheckKind::Nth(n) => is_nth(m.get_node(), *n),
            CheckKind::Field(name) => in_field(m.get_node(), name),
            // A macro's contents are inside the macro call, so never the
            // file's own items, whatever the wrapper's tree says.
            CheckKind::Top => frame.is_none() && is_top(m.get_node()),
            CheckKind::In { var, set, field } => {
                let text = if var.multi {
                    let nodes = env.get_multiple_matches(&var.name);
                    match (nodes.first(), nodes.last()) {
                        (Some(first), Some(last)) => {
                            source[first.range().start..last.range().end].to_string()
                        }
                        _ => String::new(),
                    }
                } else {
                    env.get_match(&var.name)
                        .map_or_default(|n| n.text().into_owned())
                };
                colab_core::facts::contains(set, field, &text)
            }
            CheckKind::Semantic { var, query } => {
                // An empty sequence has no code to ask about. In a macro's
                // contents the server is asked about the file it can see.
                semantic_span(var, m).is_some_and(|span| match frame {
                    Some(f) => colab_core::semantic::ask(f.file, f.to_file(span), query),
                    None => colab_core::semantic::ask(source, span, query),
                })
            }
        };
        result != self.negated
    }
}

/// The code a semantic check asks about: the binding's span (a sequence's
/// from its first node to its last), or `None` for an empty sequence.
fn semantic_span(var: &Var, m: &NodeMatch<'_, StrDoc<TsLang>>) -> Option<std::ops::Range<usize>> {
    let env = m.get_env();
    if var.multi {
        let nodes = env.get_multiple_matches(&var.name);
        match (nodes.first(), nodes.last()) {
            (Some(first), Some(last)) => Some(first.range().start..last.range().end),
            _ => None,
        }
    } else {
        env.get_match(&var.name).map(ast_grep_core::Node::range)
    }
}

pub(super) fn require_bound(bound: &[Var], var: &Var) -> Result<(), String> {
    match bound.iter().find(|b| b.name == var.name) {
        Some(b) if b.multi == var.multi => Ok(()),
        Some(b) => Err(format!(
            "`{var}` is written `{b}` in the pattern; use the same sigil"
        )),
        None if bound.is_empty() => Err(format!(
            "`{var}` is not a metavariable of the pattern, which binds none"
        )),
        None => {
            let names: Vec<String> = bound.iter().map(ToString::to_string).collect();
            Err(format!(
                "`{var}` is not a metavariable of the pattern; it binds {}",
                names.join(", ")
            ))
        }
    }
}

/// An `inside` / `has` pattern, compiled in the same language. Its
/// metavariables are its own: they are not unified with the outer
/// pattern's.
pub(super) fn nested(
    language: &PatternLanguage,
    keyword: &str,
    text: &str,
) -> Result<Forms, String> {
    let compiled = StructuralPattern::compile(language, text)
        .map_err(|e| format!("`{keyword} '{text}'`: {e}"))?;
    let mut forms = vec![compiled.pattern];
    forms.extend(compiled.also.into_iter().map(|alt| alt.pattern));
    Ok(Forms(forms))
}

/// Every visible named node kind the grammar defines.
pub(super) fn named_kinds(grammar: &tree_sitter::Language) -> Vec<&str> {
    let mut kinds: Vec<&str> = (0..u16::try_from(grammar.node_kind_count()).unwrap_or(u16::MAX))
        .filter(|&id| grammar.node_kind_is_named(id) && grammar.node_kind_is_visible(id))
        .filter_map(|id| grammar.node_kind_for_id(id))
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
}

/// The body of every source backend's
/// [`LanguageBackend::build_rule_where`]: a block without constraints is
/// an ordinary [`LanguageBackend::build_rule`]. On `pattern` (with
/// `replace` or `delete`) constraints are compiled by `compile` (the
/// backend's own `PatternOp::compile`); on `symbol` and `call` they may
/// only be `inside` / `not inside`, a syntactic scope ([`crate::scope`]).
pub fn build_rule_where<B, F>(
    backend: &B,
    module: &str,
    spec: RuleSpec,
    constraints: &[Constraint],
    compile: F,
) -> colab_core::Result<Box<dyn Operation>>
where
    B: LanguageBackend + ?Sized,
    F: FnOnce(&str, PatternAction, &[Constraint]) -> colab_core::Result<Box<dyn Operation>>,
{
    if constraints.is_empty() {
        return backend.build_rule(module, spec);
    }
    // `symbol` and `call` take a syntactic scope: `where inside '<pattern>'`.
    if matches!(module, "symbol" | "call") {
        let inner = backend.build_rule(module, spec)?;
        let scoped = crate::scope::Scoped::new(backend.lang(), module, inner, constraints, |p| {
            compile_scope(backend, p)
        })?;
        return Ok(Box::new(scoped));
    }
    if module != "pattern" {
        return Err(colab_core::backend::where_unsupported(
            backend.lang(),
            module,
        ));
    }
    let action = match &spec {
        RuleSpec::Replace { replacement, .. } => PatternAction::Replace(replacement.clone()),
        RuleSpec::Delete { .. } => PatternAction::Delete,
        RuleSpec::Create { path, template, .. } => PatternAction::Create {
            path: path.clone(),
            template: template.clone(),
        },
        _ => return Err(backend.unsupported(module, &spec)),
    };
    compile(spec.target(), action, constraints)
}

/// A scope pattern, compiled as the backend's own `<lang>::pattern` (with
/// `delete`, never applied: only its matches are used).
pub(super) fn compile_scope<B: LanguageBackend + ?Sized>(
    backend: &B,
    pattern: &str,
) -> colab_core::Result<Box<dyn Operation>> {
    backend.build_rule(
        "pattern",
        RuleSpec::Delete {
            target: pattern.to_string(),
        },
    )
}
