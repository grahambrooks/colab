//! Rewriting colab's own matches with a template: the same rendering as
//! `pattern/rewrite.rs` (the ast-grep engine's), with the template filled
//! by colab rather than by ast-grep's `TemplateFix`.
//!
//! Each match's bindings are rewritten first (nested matches inside them),
//! then the template is filled with a mark per binding, laid out against
//! the match with each binding's span marked the same way
//! ([`keep_layout`]), and each mark replaced by the binding's text, its
//! later lines moved with the line it now sits on. A match's modifiers the
//! pattern left out (`Match::absorbed`) are given back to a template that
//! is the same kind of item and spells none.

use colab_syntax::pattern::{Binding, Match, PatternNode};

use super::{Checks, EngineForm, Forms, Role};
use crate::pattern::{
    PatternLanguage, indent_with_tabs, keep_layout, only_metavariables, qualify_call, root_path,
    same_but_whitespace, shift_lines, shorthand_template, strip_blank_line_indent, tighten,
    transformable_template,
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
    /// The binding's span in the source; `None` for an empty sequence.
    span: Option<std::ops::Range<usize>>,
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
                }
            })
            .collect();
        // A sequence that matched nothing leaves no gap behind it.
        let empty: Vec<&str> = values
            .iter()
            .filter(|v| v.span.is_none())
            .map(|v| v.name.as_str())
            .collect();
        let tight = tighten(self.template, &empty);
        let template = tight.as_deref().unwrap_or(self.template);
        let start = m.node.start();
        // A node that owns the newline ending it keeps that newline.
        let end = start + source[start..m.node.end()].trim_end().len();
        let text = self.lay_out(template, &values, start..end);
        let text = match (self.gives_back, m.absorbed.first(), m.absorbed.last()) {
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
                v.span
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
            let filled = indent_with_tabs(source, range.start, &filled);
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
        // An empty sequence is no text at all; a binding with a span is
        // its mark.
        let marked = fill(template, |n| {
            let (k, v) = values.iter().enumerate().find(|(_, v)| v.name == n)?;
            match v.span {
                Some(_) => mark(k).map(String::from),
                None => Some(String::new()),
            }
        });
        let marked = indent_with_tabs(source, range.start, &marked);
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
/// rendered, none that changes only whitespace.
fn edits(
    language: &PatternLanguage,
    form: &EngineForm,
    checks: &Checks,
    source: &str,
    template: &str,
) -> Vec<Edit> {
    let Some(tree) = super::tree(&language.grammar, source) else {
        return Vec::new();
    };
    let mut matches = form.pattern.find_all(&tree);
    checks.retain(&mut matches, source, None);
    if matches.is_empty() {
        return Vec::new();
    }
    let written = template_for(form, language, template);
    let (filled, _) = transformable_template(&written);
    let mut order: Vec<(std::ops::Range<usize>, usize)> = matches
        .iter()
        .enumerate()
        .map(|(i, m)| (m.node.range(), i))
        .collect();
    order.sort_by_key(|(r, i)| (r.start, std::cmp::Reverse(r.end), *i));
    let rewriter = Rewriter {
        source,
        matches: &matches,
        order,
        template: &filled,
        gives_back: gives_back(language, form, &written),
    };
    let mut edits: Vec<Edit> = rewriter
        .outermost(0..source.len(), None)
        .into_iter()
        .map(|i| rewriter.render(i))
        .filter(|e| !same_but_whitespace(&source[e.start..e.end], &e.text))
        .collect();
    // A template that only unwraps the match takes its attached docs and
    // attributes with it, as a delete does.
    let leading = language.leading;
    let attaches = !(leading.kinds.is_empty() && leading.doc_comments.is_empty());
    if attaches
        && only_metavariables(&written)
        && let Some(ts) = crate::parse_cache::parse(&language.grammar, source)
    {
        for edit in &mut edits {
            let start = crate::pattern::attached_start(&ts, source, edit.start, edit.end, leading);
            edit.start = edit.start.min(start);
        }
    }
    edits
}

/// Every match of `forms` that `checks` holds for in `source`, replaced by
/// `template`: each form in turn over the text the one before left, its
/// matches' bindings rewritten first so one pass reaches the fixed point.
pub fn rewrite(
    language: &PatternLanguage,
    forms: &Forms,
    checks: &Checks,
    source: &str,
    template: &str,
) -> String {
    let mut text = source.to_string();
    for form in &forms.forms {
        let made = edits(language, form, checks, &text, template);
        if !made.is_empty() {
            text = apply_edits(&text, made);
        }
    }
    // A rewrite that leaves code which parsed unparseable is not made (the
    // rule explains why it changed nothing).
    let breaks = |t: &str| super::tree(&language.grammar, t).is_none_or(|t| t.has_error());
    if text != source && breaks(&text) && !breaks(source) {
        return source.to_string();
    }
    text
}
