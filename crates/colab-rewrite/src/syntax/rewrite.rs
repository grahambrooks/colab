//! Rewriting colab's own matches with a template, filled by colab (the
//! layout helpers are `pattern/rewrite.rs`'s).
//!
//! Each match's bindings are rewritten first (nested matches inside them),
//! then the template is filled with a mark per binding, laid out against
//! the match with each binding's span marked the same way
//! ([`keep_layout`]), and each mark replaced by the binding's text, its
//! later lines moved with the line it now sits on. A match's modifiers the
//! pattern left out (`Match::absorbed`) are given back to a template that
//! is the same kind of item and spells none.

use colab_syntax::pattern::{Binding, Match, PatternNode};

use super::{Checks, EngineForm, Forms, Frame, Role};
use crate::pattern::{
    PatternLanguage, indent_later_lines, keep_layout, keep_modifier_words, only_metavariables,
    qualify_call, root_path, same_but_whitespace, shift_lines, shorthand_template,
    strip_blank_line_indent, tighten, transformable_template,
};
use crate::{Edit, apply_edits};

/// First of the private-use characters bindings are marked with.
const SENTINEL: u32 = 0xF0000;

/// `template` (`$NAME` only, as [`transformable_template`] writes it) with
/// each name `value` knows replaced; other `$` text kept.
fn fill(template: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let len = after
            .find(|c: char| !(c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()))
            .unwrap_or(after.len());
        match value(&after[..len]).filter(|_| len > 0) {
            Some(text) => {
                out.push_str(&text);
                rest = &after[len..];
            }
            None => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The template a qualified form's match is filled with: the matched path
/// before the template's own bare callee when it is the same function or a
/// rename of it; a template that wraps the call is used as written.
fn requalify(template: &str, separator: &str, callee: &str, rooted: bool) -> Option<String> {
    let (qualified, called) = qualify_call(template, separator)?;
    let renamed = !template.contains(callee);
    let qualified = if rooted {
        root_path(&qualified, separator)
    } else {
        qualified
    };
    (called == callee || renamed).then_some(qualified)
}

struct Value {
    name: String,
    text: String,
    /// The binding's span in the source (for a `let`'s value, that of the
    /// capture it derives from, whose line its later lines move with);
    /// `None` for an empty sequence.
    span: Option<std::ops::Range<usize>>,
    /// `span` is this value's own text, not a `let`'s source.
    own: bool,
}

/// One compiled `let`: `var` is `from`'s text through `steps`, after a
/// first `rewrite(…)` step's rewriters, each a pattern's forms and its
/// template, rewrote the code `from` binds.
struct Derived {
    var: String,
    from: String,
    rewriters: Vec<(Forms, String)>,
    steps: Vec<crate::derive::Step>,
}

impl Derived {
    fn apply(&self, text: &str) -> String {
        self.steps
            .iter()
            .fold(text.to_string(), |text, step| step.apply(&text))
    }
}

/// A rule's `let`s (`colab_core::Test::Let` constraints), in order.
#[derive(Default)]
pub struct Lets(Vec<Derived>);

impl Lets {
    /// The `let`s among `constraints`, for a pattern binding `bound`. Each
    /// derives from a metavariable the pattern binds or an earlier `let`,
    /// and names a new `$VAR`, written with one `$`: it holds text, not
    /// nodes.
    pub fn compile(
        language: &PatternLanguage,
        bound: &[colab_core::Var],
        constraints: &[colab_core::Constraint],
    ) -> Result<Self, String> {
        let mut out: Vec<Derived> = Vec::new();
        for c in constraints {
            let colab_core::Test::Let { var, from, steps } = &c.test else {
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
                crate::pattern::require_bound(bound, from).map_err(|e| format!("`{c}`: {e}"))?;
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
                            crate::pattern::unbound_in_template(&r.pattern, &r.template, &[])
                                .and_then(|()| super::compile_forms(language, &r.pattern))
                                .map(|forms| (forms, r.template.clone()))
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
            out.push(Derived {
                var: var.name.clone(),
                from: from.name.clone(),
                rewriters,
                steps: rest
                    .iter()
                    .map(crate::derive::Step::compile)
                    .collect::<Result<_, _>>()?,
            });
        }
        Ok(Self(out))
    }
}

struct Rewriter<'a, 't> {
    source: &'a str,
    matches: &'a [Match<'t>],
    /// `matches`' ranges by start, wider first, then index.
    order: Vec<(std::ops::Range<usize>, usize)>,
    template: &'a str,
    /// Whether a match's absorbed modifiers go back in front of the
    /// template's text.
    gives_back: bool,
    /// The language's modifier keywords and the pattern's head (before
    /// its first `(` or `{`), for a template that leaves some out; `None`
    /// without any, or for a template that only unwraps.
    keep: Option<(&'static [&'static str], &'a str)>,
    /// The kinds that belong to the item after them ([`crate::pattern::Leading`]).
    leading: &'static [&'static str],
    lets: &'a [Derived],
    /// The rewriter of each `let` with a `rewrite` step, by position: one
    /// per form its patterns compiled to (empty without one).
    subs: &'a [Vec<Rewriter<'a, 't>>],
}

/// `matches`' ranges in the order [`Rewriter::outermost`] sweeps.
fn order_of(matches: &[Match<'_>]) -> Vec<(std::ops::Range<usize>, usize)> {
    let mut order: Vec<_> = matches
        .iter()
        .enumerate()
        .map(|(i, m)| (m.node.range(), i))
        .collect();
    order.sort_by_key(|(r, i)| (r.start, std::cmp::Reverse(r.end), *i));
    order
}

/// `source[span]` with every outermost match of any of `forms` (one
/// rewriter's patterns) rewritten, the text between kept; of two
/// overlapping edits the outer one is made.
fn rewrite_with(forms: &[Rewriter<'_, '_>], source: &str, span: std::ops::Range<usize>) -> String {
    let mut edits: Vec<Edit> = forms
        .iter()
        .flat_map(|form| {
            form.outermost(span.clone(), None)
                .into_iter()
                .map(|i| form.render(i))
        })
        .collect();
    edits.sort_by_key(|e| (e.start, std::cmp::Reverse(e.end)));
    let mut out = String::new();
    let mut at = span.start;
    for edit in edits {
        if edit.start < at {
            continue;
        }
        out.push_str(&source[at..edit.start]);
        out.push_str(&edit.text);
        at = edit.end;
    }
    out.push_str(&source[at..span.end]);
    out
}

impl Rewriter<'_, '_> {
    /// Indices of the matches inside `span` that no other match inside it
    /// contains, in source order (`outer` excluded).
    fn outermost(&self, span: std::ops::Range<usize>, outer: Option<usize>) -> Vec<usize> {
        let first = self.order.partition_point(|(r, _)| r.start < span.start);
        let mut out = Vec::new();
        let mut reach: Option<usize> = None;
        for (r, i) in &self.order[first..] {
            if r.start > span.end {
                break;
            }
            if r.end > span.end || Some(*i) == outer || reach.is_some_and(|end| r.end <= end) {
                continue;
            }
            reach = Some(reach.map_or(r.end, |end| end.max(r.end)));
            out.push(*i);
        }
        out
    }

    /// `source[span]` with each outermost match inside it rewritten.
    fn rewrite_span(&self, span: std::ops::Range<usize>, outer: usize) -> String {
        let mut out = String::new();
        let mut at = span.start;
        for i in self.outermost(span.clone(), Some(outer)) {
            let edit = self.render(i);
            if edit.start < at {
                continue;
            }
            out.push_str(&self.source[at..edit.start]);
            out.push_str(&edit.text);
            at = edit.end;
        }
        out.push_str(&self.source[at..span.end]);
        out
    }

    fn render(&self, i: usize) -> Edit {
        let source = self.source;
        let m = &self.matches[i];
        let values: Vec<Value> = m
            .bindings
            .iter()
            .map(|(name, binding)| {
                let span = match binding {
                    Binding::One(n) => Some(n.range()),
                    // A sequence can end in a terminator that is only
                    // whitespace (Go's newline after a statement).
                    Binding::Many(nodes) => match (nodes.first(), nodes.last()) {
                        (Some(first), Some(last)) => {
                            let (start, end) = (first.start(), last.end());
                            Some(start..start + source[start..end].trim_end().len())
                        }
                        _ => None,
                    },
                };
                let text = span
                    .clone()
                    .map_or_else(String::new, |s| self.rewrite_span(s, i));
                Value {
                    name: name.clone(),
                    text,
                    span,
                    own: true,
                }
            })
            .collect();
        // A sequence that matched nothing leaves no gap behind it.
        let empty: Vec<String> = values
            .iter()
            .filter(|v| v.span.is_none())
            .map(|v| v.name.clone())
            .collect();
        let empty: Vec<&str> = empty.iter().map(String::as_str).collect();
        // Each `let`'s value, laid out with the capture it comes from.
        let mut values = values;
        for (k, derived) in self.lets.iter().enumerate() {
            let Some(base) = values.iter().find(|v| v.name == derived.from) else {
                continue;
            };
            let own_span = values
                .iter()
                .find(|v| v.own && v.name == derived.from)
                .and_then(|v| v.span.clone());
            let start = match (self.subs.get(k), &own_span) {
                (Some(forms), Some(span)) if !forms.is_empty() => {
                    rewrite_with(forms, source, span.clone())
                }
                _ => base.text.clone(),
            };
            let value = Value {
                name: derived.var.clone(),
                text: derived.apply(&start),
                span: base.span.clone(),
                own: false,
            };
            values.push(value);
        }
        let tight = tighten(self.template, &empty);
        let template = tight.as_deref().unwrap_or(self.template);
        // A node that holds its own doc comments and the attributes the
        // pattern left out (rust-analyzer's items) keeps them where they
        // are: the edit starts after them, as it does where they are the
        // item's siblings (tree-sitter's).
        let start = m
            .node
            .children()
            .filter(|c| !c.is_trivia())
            .find(|c| {
                !(c.is_extra() || (self.leading.contains(&c.kind_name()) && m.absorbed.contains(c)))
            })
            .map_or(m.node.start(), |c| c.start());
        // A node that owns the newline ending it keeps that newline.
        let end = start + source[start..m.node.end()].trim_end().len();
        let text = self.lay_out(template, &values, start..end);
        let absorbed: Vec<_> = m.absorbed.iter().filter(|a| a.start() >= start).collect();
        let text = match (self.gives_back, absorbed.first(), absorbed.last()) {
            (true, Some(first), Some(last)) => {
                // Through the space after the last one, up to what follows.
                let next =
                    std::iter::successors(last.next_sibling(), colab_syntax::Node::next_sibling)
                        .find(|n| !n.is_trivia())
                        .map_or(last.end(), |n| n.start());
                format!("{}{text}", &source[first.start()..next])
            }
            _ => text,
        };
        // The modifier keywords the template leaves out, put back.
        let text = match self.keep {
            Some((kinds, head)) => {
                let mods: Vec<&str> = m
                    .node
                    .children()
                    .filter(|c| !c.is_trivia() && !c.is_extra() && c.start() >= start)
                    .take_while(|c| kinds.contains(&c.kind_name()))
                    .map(|c| c.text())
                    .collect();
                keep_modifier_words(&mods, text, head)
            }
            None => text,
        };
        Edit::new(start, end, text)
    }

    /// The template filled for the match over `source[range]`, laid out
    /// against it: each binding is one mark while the layout is kept, then
    /// its text, verbatim, its later lines moved with its new line.
    fn lay_out(&self, template: &str, values: &[Value], range: std::ops::Range<usize>) -> String {
        let source = self.source;
        let old = &source[range.clone()];
        let mark = |k: usize| {
            u32::try_from(k)
                .ok()
                .and_then(|k| char::from_u32(SENTINEL + k))
        };
        let sentinel = |c: char| (SENTINEL..SENTINEL + 0x100).contains(&u32::from(c));
        let mut inside: Vec<(usize, &Value)> = values
            .iter()
            .enumerate()
            .filter(|(_, v)| {
                v.own
                    && v.span
                        .as_ref()
                        .is_some_and(|s| s.start >= range.start && s.end <= range.end)
            })
            .collect();
        inside.sort_by_key(|(_, v)| v.span.as_ref().map_or(0, |s| s.start));
        let overlap = inside.windows(2).any(|w| {
            let (a, b) = (w[0].1.span.as_ref(), w[1].1.span.as_ref());
            a.zip(b).is_some_and(|(a, b)| a.end > b.start)
        });
        if old.chars().any(sentinel) || overlap || values.len() >= 0x100 {
            // Plain: fill with the texts and keep what layout matches.
            let filled = fill(template, |n| {
                values.iter().find(|v| v.name == n).map(|v| v.text.clone())
            });
            let filled = indent_later_lines(source, range.start, &filled);
            return keep_layout(old, &strip_blank_line_indent(&filled));
        }
        let mut skeleton = String::with_capacity(old.len());
        let mut at = range.start;
        for (k, v) in &inside {
            let span = v.span.clone().unwrap_or_default();
            skeleton.push_str(&source[at..span.start]);
            skeleton.extend(mark(*k));
            at = span.end;
        }
        skeleton.push_str(&source[at..range.end]);
        // A value with a span is its mark; one without (an empty sequence,
        // or a `let` of one) is its text.
        let marked = fill(template, |n| {
            let (k, v) = values.iter().enumerate().find(|(_, v)| v.name == n)?;
            match v.span {
                Some(_) => mark(k).map(String::from),
                None => Some(v.text.clone()),
            }
        });
        let marked = indent_later_lines(source, range.start, &marked);
        let laid = keep_layout(&skeleton, &strip_blank_line_indent(&marked));
        let indent = |line: &str| line.chars().take_while(|c| c.is_whitespace()).count();
        let line_start = source[..range.start].rfind('\n').map_or(0, |p| p + 1);
        let lead = indent(&source[line_start..range.start]);
        let mut out = String::with_capacity(laid.len() + old.len());
        for ch in laid.chars() {
            let Some((_, v)) = values
                .iter()
                .enumerate()
                .find(|(k, _)| mark(*k) == Some(ch))
            else {
                out.push(ch);
                continue;
            };
            let span = v.span.clone().unwrap_or_default();
            let here = out.rfind('\n').map_or(lead, |p| indent(&out[p + 1..]));
            let from = source[..span.start].rfind('\n').map_or(0, |p| p + 1);
            let was = indent(&source[from..span.start]);
            out.push_str(&shift_lines(
                &v.text,
                isize::try_from(here).unwrap_or(0) - isize::try_from(was).unwrap_or(0),
            ));
        }
        out
    }
}

/// Whether a template, read as a pattern, is the item `form` matches and
/// spells none of its modifiers, so a match's are given back to it.
fn gives_back(language: &PatternLanguage, form: &EngineForm, template: &str) -> bool {
    if only_metavariables(template) || form.pattern.rules().absorb.is_empty() {
        return false;
    }
    let Ok(read) = super::compile_forms(language, template) else {
        return false;
    };
    // Any reading of it: C#'s `void New() {}` is a local function first,
    // and a method in a class.
    read.forms.iter().any(|f| {
        let pattern = &f.pattern;
        let spells = match pattern.root() {
            PatternNode::Node { children, .. } => children.iter().any(|c| match c {
                PatternNode::Node { kind, .. } | PatternNode::Token { kind, .. } => {
                    form.pattern.rules().absorb.contains(kind)
                }
                PatternNode::Var(_) => false,
            }),
            _ => true,
        };
        pattern.kind().is_some() && pattern.kind() == form.pattern.kind() && !spells
    })
}

/// What a rewrite with `template` needs to keep a match's modifier
/// keywords: the language's list and the pattern's head (before its first
/// `(` or `{`). `None` without a list, and for a template that is one
/// metavariable, which unwraps the item and leaves nothing to put them on.
fn kept_modifiers<'p>(
    language: &PatternLanguage,
    template: &str,
    pattern_text: &'p str,
) -> Option<(&'static [&'static str], &'p str)> {
    let tokens = language.modifier_tokens;
    let only = template.trim();
    let unwraps = only.strip_prefix('$').is_some_and(|name| {
        !name.is_empty()
            && name
                .trim_start_matches('$')
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    });
    if tokens.is_empty() || unwraps {
        return None;
    }
    let head = &pattern_text[..pattern_text.find(['(', '{']).unwrap_or(pattern_text.len())];
    Some((tokens, head))
}

/// The template a form fills: requalified, shorthand, without the `;` its
/// node does not hold.
fn template_for(form: &EngineForm, language: &PatternLanguage, template: &str) -> String {
    let mut template = template.to_string();
    if let Role::Qualified {
        separator,
        callee,
        rooted,
    } = &form.role
        && let Some(t) = requalify(&template, separator, callee, *rooted)
    {
        template = t;
    }
    if let (Role::Shorthand { fields }, Some(shorthand)) = (&form.role, language.shorthand_fields) {
        template = shorthand_template(&template, fields, shorthand);
    }
    if form.loose_semicolon
        && let Some(t) = template.trim_end().strip_suffix(';')
    {
        template = t.to_string();
    }
    template
}

/// The edits one form makes to `source`: its outermost matches, each
/// rendered, none that changes only whitespace. `pattern_text` is the
/// rule's pattern as written.
#[allow(clippy::too_many_arguments)] // one rule's parts, not an API
fn edits(
    language: &PatternLanguage,
    form: &EngineForm,
    pattern_text: &str,
    checks: &Checks,
    lets: &Lets,
    source: &str,
    template: &str,
    frame: Option<&Frame<'_>>,
) -> Vec<Edit> {
    let Some(tree) = super::source_tree(language, source) else {
        return Vec::new();
    };
    let mut matches = form.pattern.find_all(&tree);
    checks.retain(&mut matches, source, frame);
    if matches.is_empty() {
        return Vec::new();
    }
    let written = template_for(form, language, template);
    let (filled, _) = transformable_template(&written);
    // Each `let`'s rewriters: for every form of each, its template and its
    // matches in this source; one list per `let`, empty without one.
    let parts: Vec<Vec<(String, Vec<Match<'_>>)>> = lets
        .0
        .iter()
        .map(|d| {
            d.rewriters
                .iter()
                .flat_map(|(forms, template)| {
                    let (template, _) = transformable_template(template);
                    let tree = &tree;
                    forms
                        .forms
                        .iter()
                        .map(move |f| (template.clone(), f.pattern.find_all(tree)))
                })
                .collect()
        })
        .collect();
    let subs: Vec<Vec<Rewriter<'_, '_>>> = parts
        .iter()
        .map(|forms| {
            forms
                .iter()
                .map(|(template, matches)| Rewriter {
                    source,
                    matches,
                    order: order_of(matches),
                    template,
                    gives_back: false,
                    keep: None,
                    leading: language.leading.kinds,
                    lets: &[],
                    subs: &[],
                })
                .collect()
        })
        .collect();
    let rewriter = Rewriter {
        source,
        matches: &matches,
        order: order_of(&matches),
        template: &filled,
        gives_back: gives_back(language, form, &written),
        keep: kept_modifiers(language, &written, pattern_text),
        leading: language.leading.kinds,
        lets: &lets.0,
        subs: &subs,
    };
    let mut edits: Vec<Edit> = rewriter
        .outermost(0..source.len(), None)
        .into_iter()
        .map(|i| rewriter.render(i))
        .filter(|e| !same_but_whitespace(&source[e.start..e.end], &e.text))
        .collect();
    // A template that only unwraps the match takes its attached docs and
    // attributes with it, as a delete does; a macro's contents have none.
    let leading = language.leading;
    let attaches = !(leading.kinds.is_empty() && leading.doc_comments.is_empty());
    if attaches && frame.is_none() && only_metavariables(&written) {
        for edit in &mut edits {
            let start =
                crate::pattern::attached_start(&tree, source, edit.start, edit.end, leading);
            edit.start = edit.start.min(start);
        }
    }
    edits
}

/// [`rewrite`] of `source` judged in `frame` (a macro's contents) at macro
/// nesting `depth`: each form in turn, then the macro calls in the text,
/// whose contents its tree cannot see into. A frame stays as the contents
/// were read, so a later form's semantic question in a macro asks about
/// the position before an earlier form's edit there.
#[allow(clippy::too_many_arguments)] // the recursion's state, not an API
fn rewrite_in(
    language: &PatternLanguage,
    forms: &Forms,
    checks: &Checks,
    lets: &Lets,
    source: &str,
    template: &str,
    frame: Option<&Frame<'_>>,
    depth: usize,
) -> String {
    let mut text = source.to_string();
    for form in &forms.forms {
        let made = edits(
            language,
            form,
            &forms.text,
            checks,
            lets,
            &text,
            template,
            frame,
        );
        if !made.is_empty() {
            text = apply_edits(&text, made);
        }
    }
    let moved = frame.map(|f| f.resized(source.len(), text.len()));
    super::edit_macros(
        language,
        forms,
        checks,
        &text,
        moved.as_ref(),
        depth,
        |wrapped, inner, d| {
            rewrite_in(
                language,
                forms,
                checks,
                lets,
                wrapped,
                template,
                Some(inner),
                d,
            )
        },
    )
}

/// Every match of `forms` that `checks` holds for in `source`, replaced by
/// `template`: each form in turn over the text the one before left, its
/// matches' bindings rewritten first so one pass reaches the fixed point,
/// then inside the macro calls the language reads.
pub fn rewrite(
    language: &PatternLanguage,
    forms: &Forms,
    checks: &Checks,
    lets: &Lets,
    source: &str,
    template: &str,
) -> String {
    let text = rewrite_unguarded(language, forms, checks, lets, source, template);
    // A rewrite that leaves code which parsed unparseable is not made (the
    // rule explains why it changed nothing).
    let breaks = |t: &str| super::source_tree(language, t).is_none_or(|t| t.has_error());
    if text != source && breaks(&text) && !breaks(source) {
        return source.to_string();
    }
    text
}

/// [`rewrite`] without refusing a result that does not parse: what a rule
/// explains a skipped file by.
pub(crate) fn rewrite_unguarded(
    language: &PatternLanguage,
    forms: &Forms,
    checks: &Checks,
    lets: &Lets,
    source: &str,
    template: &str,
) -> String {
    rewrite_in(language, forms, checks, lets, source, template, None, 0)
}

/// Where the first edit whose rewrite alone leaves `source` (which parses)
/// unparseable starts: the site a "would not parse" skip is about. `None`
/// when no single edit breaks it, only their combination.
pub(crate) fn first_breaking_edit(
    language: &PatternLanguage,
    forms: &Forms,
    checks: &Checks,
    lets: &Lets,
    source: &str,
    template: &str,
) -> Option<usize> {
    let breaks = |t: &str| super::source_tree(language, t).is_none_or(|t| t.has_error());
    forms.forms.iter().find_map(|form| {
        edits(
            language,
            form,
            &forms.text,
            checks,
            lets,
            source,
            template,
            None,
        )
        .into_iter()
        .find(|edit| breaks(&apply_edits(source, vec![edit.clone()])))
        .map(|edit| edit.start)
    })
}
