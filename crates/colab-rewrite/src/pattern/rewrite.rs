//! Rewriting a match with a template: metavariable substitution, keeping
//! the matched code's layout and indentation, and nested rewriters.

use super::*;

/// Fills a template for one match after rewriting the matches nested in
/// its bindings — see [`StructuralPattern::rewrite`].
pub(super) struct Rewriter<'a, 't> {
    pub(super) pattern: &'a Pattern,
    pub(super) fix: &'a TemplateFix,
    pub(super) lets: &'a [Derived],
    /// The rewriter of each `let` that has a `rewrite` step, by position:
    /// one per form its pattern compiled to (empty without one).
    pub(super) subs: &'a [Vec<Rewriter<'a, 't>>],
    /// Every match in the source, nested ones included.
    pub(super) matches: &'a [NodeMatch<'t, StrDoc<TsLang>>],
    /// The template `fix` was built from, its language and names, so a
    /// match whose sequence capture is empty can be filled without the
    /// separator around it ([`tighten`]). `None` keeps `fix` for all.
    pub(super) template: Option<(&'a str, &'a TsLang, &'a [String])>,
    /// The language's modifiers ([`PatternLanguage::modifier_tokens`]) and
    /// the pattern's head, so a rewrite puts back a match's modifiers that
    /// the template leaves out ([`keep_modifier_tokens`]). `None` for none.
    pub(super) keep: Option<(&'static [&'static str], &'a str)>,
    /// `matches`' ranges with their indices, by start, then the wider
    /// first, then the earlier match ([`match_order`]), so
    /// [`Self::outermost`] is one sweep, not a comparison of every pair.
    pub(super) order: Vec<(std::ops::Range<usize>, usize)>,
}

/// The ranges of `matches` in the order [`Rewriter::outermost`] sweeps.
pub(super) fn match_order(
    matches: &[NodeMatch<'_, StrDoc<TsLang>>],
) -> Vec<(std::ops::Range<usize>, usize)> {
    let mut order: Vec<_> = matches
        .iter()
        .enumerate()
        .map(|(i, m)| (m.range(), i))
        .collect();
    order.sort_by_key(|(r, i)| (r.start, std::cmp::Reverse(r.end), *i));
    order
}

impl Rewriter<'_, '_> {
    /// Indices of the matches inside `span` that no other match inside it
    /// contains, in source order. `outer` is the match whose binding
    /// `span` is, so it does not count as nested in itself; of several
    /// matches over the same bytes (a call and its expression statement),
    /// the first is kept.
    pub(super) fn outermost(
        &self,
        span: std::ops::Range<usize>,
        outer: Option<usize>,
    ) -> Vec<usize> {
        // In `order`, every match that contains another inside `span`
        // comes before it, so a match is covered exactly when one before
        // it inside the span reaches as far.
        let first = self.order.partition_point(|(r, _)| r.start < span.start);
        let mut out = Vec::new();
        let mut reach: Option<usize> = None;
        for (r, i) in &self.order[first..] {
            if r.start > span.end {
                break;
            }
            if r.end > span.end || Some(*i) == outer {
                continue;
            }
            if reach.is_some_and(|end| r.end <= end) {
                continue;
            }
            reach = Some(reach.map_or(r.end, |end| end.max(r.end)));
            out.push(*i);
        }
        out
    }

    /// `source[span]` with each outermost match inside it rewritten.
    fn rewrite_span(&self, source: &str, span: std::ops::Range<usize>, outer: usize) -> String {
        let mut out = String::new();
        let mut at = span.start;
        for i in self.outermost(span.clone(), Some(outer)) {
            let edit = self.render(source, i);
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

    /// The edit replacing match `i`, its bindings rewritten first.
    pub(super) fn render(&self, source: &str, i: usize) -> Edit {
        let mut nm = self.matches[i].clone();
        let vars: Vec<MetaVariable> = nm.get_env().get_matched_variables().collect();
        // Each binding's (rewritten) text and source span, for the `let`s
        // derived from it.
        type Value = (String, MetaVariable, String, Option<std::ops::Range<usize>>);
        let mut values: Vec<Value> = Vec::new();
        for var in vars {
            let (name, span) = match &var {
                MetaVariable::Capture(name, _) => {
                    let span = nm.get_env().get_match(name).map(ast_grep_core::Node::range);
                    (name.clone(), span)
                }
                MetaVariable::MultiCapture(name) => {
                    let nodes = nm.get_env().get_multiple_matches(name);
                    let span = match (nodes.first(), nodes.last()) {
                        // A sequence can end in a terminator that is only
                        // whitespace (Go's newline after a statement);
                        // the template's own text follows it instead.
                        (Some(first), Some(last)) => {
                            let (start, end) = (first.range().start, last.range().end);
                            Some(start..start + source[start..end].trim_end().len())
                        }
                        _ => None,
                    };
                    (name.clone(), span)
                }
                _ => continue,
            };
            let text = span
                .clone()
                .map_or_default(|span| self.rewrite_span(source, span, i));
            nm.get_env_mut()
                .insert_transformation(&var, &name, text.clone().into_bytes());
            values.push((name, var, text, span));
        }
        // Each `let`'s value, with the source span of the capture it comes
        // from (through earlier `let`s): its layout follows that capture's.
        let mut derived_values: Vec<Value> = Vec::new();
        for (k, derived) in self.lets.iter().enumerate() {
            let Some((_, var, text, span)) = values
                .iter()
                .chain(&derived_values)
                .find(|(n, ..)| *n == derived.from)
            else {
                continue;
            };
            let own_span = values
                .iter()
                .find(|(n, ..)| *n == derived.from)
                .and_then(|(.., span)| span.clone());
            // A rewriter starts from the binding's source: each of its
            // matches there rewritten, the text between them kept.
            let start = match (self.subs.get(k), &own_span) {
                (Some(forms), Some(span)) if !forms.is_empty() => {
                    rewrite_with(forms, source, span.clone())
                }
                _ => text.clone(),
            };
            let (var, text, base) = (var.clone(), derived.apply(&start), span.clone());
            nm.get_env_mut()
                .insert_transformation(&var, &derived.var, text.clone().into_bytes());
            derived_values.push((derived.var.clone(), var, text, base));
        }
        // A sequence that matched nothing leaves no gap behind it.
        let empty: Vec<&str> = values
            .iter()
            .filter(|(_, var, _, span)| {
                matches!(var, MetaVariable::MultiCapture(_)) && span.is_none()
            })
            .map(|(name, ..)| name.as_str())
            .collect();
        let tight = match self.template {
            Some((template, lang, names)) if !empty.is_empty() => {
                tighten(template, &empty).map(|t| TemplateFix::with_transform(&t, lang, names))
            }
            _ => None,
        };
        let fix = tight.as_ref().unwrap_or(self.fix);
        let edit = nm.make_edit(self.pattern, fix);
        let (start, end) = (edit.position, edit.position + edit.deleted_length);
        // A node that owns the newline ending it (Go's `expression_case`)
        // keeps that newline: the template replaces the code only.
        let end = start + source[start..end].trim_end().len();
        // Each capture with a source span is laid out as one token and put
        // back verbatim, so the layout diff never borrows one capture's
        // line breaks for another's equal tokens (`swap($A, $B)` →
        // `swapped($B, $A)` must not spread a one-line `$B` over lines).
        // A `let`'s value is marked too, and laid out against the capture
        // it is derived from: only that capture's span is in the source.
        let captures: Vec<Capture> = values
            .iter()
            .map(|v| (v, true))
            .chain(derived_values.iter().map(|v| (v, false)))
            .enumerate()
            .filter_map(|(k, ((name, var, text, span), own))| {
                let span = span.clone()?;
                let mark = char::from_u32(SENTINEL + u32::try_from(k).ok()?)?;
                Some(Capture {
                    mark,
                    name: name.clone(),
                    var: var.clone(),
                    text: text.clone(),
                    span,
                    own,
                })
            })
            .collect();
        let text = match self.render_verbatim(&nm, fix, source, start..end, &captures) {
            Some(text) => text,
            None => {
                let text = String::from_utf8_lossy(&edit.inserted_text);
                let text = indent_with_tabs(source, edit.position, &text);
                keep_layout(&source[start..end], &strip_blank_line_indent(&text))
            }
        };
        let text = keep_modifier_tokens(nm.get_node(), source, text, self.keep);
        Edit::new(start, end, text)
    }

    /// The template for match `nm` over `source[range]` with each capture
    /// as a sentinel character, laid out against the match with each
    /// capture's span replaced by the same sentinel, then each sentinel
    /// replaced by the capture's text, its later lines shifted by as many
    /// columns as the capture moved. `None` (use the plain layout) when
    /// the text already holds a sentinel character, or a capture lies
    /// partly outside the match.
    fn render_verbatim(
        &self,
        nm: &NodeMatch<'_, StrDoc<TsLang>>,
        fix: &TemplateFix,
        source: &str,
        range: std::ops::Range<usize>,
        captures: &[Capture],
    ) -> Option<String> {
        let old = &source[range.clone()];
        let sentinel = |c: char| (SENTINEL..SENTINEL + 0x100).contains(&u32::from(c));
        if captures.is_empty() || old.chars().any(sentinel) {
            return None;
        }
        // A `let`'s value is not in the source; its mark is a new token in
        // the template, placed by the template's own whitespace.
        let mut inside: Vec<&Capture> = captures
            .iter()
            .filter(|c| c.own && c.span.start >= range.start && c.span.end <= range.end)
            .collect();
        inside.sort_by_key(|c| c.span.start);
        if inside.windows(2).any(|w| w[0].span.end > w[1].span.start) {
            return None;
        }
        let mut skeleton_old = String::with_capacity(old.len());
        let mut at = range.start;
        for c in &inside {
            skeleton_old.push_str(&source[at..c.span.start]);
            skeleton_old.push(c.mark);
            at = c.span.end;
        }
        skeleton_old.push_str(&source[at..range.end]);

        let mut marked = nm.clone();
        for c in captures {
            marked.get_env_mut().insert_transformation(
                &c.var,
                &c.name,
                c.mark.to_string().into_bytes(),
            );
        }
        let edit = marked.make_edit(self.pattern, fix);
        let template = String::from_utf8_lossy(&edit.inserted_text);
        let template = indent_with_tabs(source, range.start, &template);
        let laid = keep_layout(&skeleton_old, &strip_blank_line_indent(&template));

        // A capture's later lines move with the indentation of the line it
        // sits on, as the template's lines do: by how much more (or less)
        // that line is indented than the one the capture came from. Not
        // by column: `unsafe $X` moves `$X` right, not its body.
        let indent = |line: &str| line.chars().take_while(|c| c.is_whitespace()).count();
        let line_start = source[..range.start].rfind('\n').map_or(0, |p| p + 1);
        let lead = indent(&source[line_start..range.start]);
        let mut out = String::with_capacity(laid.len() + old.len());
        for ch in laid.chars() {
            let Some(c) = captures.iter().find(|c| c.mark == ch) else {
                out.push(ch);
                continue;
            };
            // Still on the match's first line, that line's indentation.
            let here = out.rfind('\n').map_or(lead, |p| indent(&out[p + 1..]));
            let from = source[..c.span.start].rfind('\n').map_or(0, |p| p + 1);
            let was = indent(&source[from..c.span.start]);
            out.push_str(&shift_lines(&c.text, here as isize - was as isize));
        }
        Some(out)
    }
}

/// First of the private-use characters [`Rewriter::render_verbatim`] marks
/// captures with, one per capture.
const SENTINEL: u32 = 0xF0000;

/// A capture as [`Rewriter::render_verbatim`] lays it out.
struct Capture {
    mark: char,
    name: String,
    var: MetaVariable,
    text: String,
    /// The capture's source span, or for a `let`'s value the span of the
    /// capture it is derived from, whose line its later lines move with.
    span: std::ops::Range<usize>,
    /// `span` is this value's own text (not a `let`'s source).
    own: bool,
}

/// `text` with every line after the first moved `by` columns: indentation
/// added (in the line's own indent character) or removed, never more than
/// a line has.
pub(crate) fn shift_lines(text: &str, by: isize) -> String {
    if by == 0 || !text.contains('\n') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            if line.trim().is_empty() {
                out.push_str(line);
                continue;
            }
            if by > 0 {
                let pad = if line.starts_with('\t') { '\t' } else { ' ' };
                out.extend(std::iter::repeat_n(pad, by.unsigned_abs()));
                out.push_str(line);
            } else {
                let indent = line.len() - line.trim_start().len();
                let mut cut = indent.min(by.unsigned_abs());
                while !line.is_char_boundary(cut) {
                    cut -= 1;
                }
                out.push_str(&line[cut..]);
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

/// `new` (a rendered template) with the whitespace of `old` (the text it
/// replaces) around every token the two share.
///
/// A template replaces the whole match in its own layout, so
/// `func ($R $T) $F() { $$$B }` → `func ($R $T) $NAME() { $$$B }` would
/// put a many-line body on one line. Tokens are aligned (quoted text is
/// one token, so a string's spaces are never mixed): an unchanged token
/// keeps the whitespace before it in `old`, unless it follows a change on
/// the same line, or the template breaks the line there; a new one keeps the whitespace
/// before it in `new`. A rewrite then changes what it changes and leaves
/// the layout around it, as a layout-only rewrite is not made at all
/// ([`same_but_whitespace`]).
pub(crate) fn keep_layout(old: &str, new: &str) -> String {
    let (a, b) = (layout_tokens(old), layout_tokens(new));
    if a.is_empty() || b.is_empty() {
        return new.to_string();
    }
    let ta: Vec<&str> = a.iter().map(|t| t.1).collect();
    let tb: Vec<&str> = b.iter().map(|t| t.1).collect();
    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &ta, &tb);
    let mut out = String::with_capacity(new.len());
    // Whose layout the line being written follows: the original's, once a
    // line of it is kept, or the template's (from the start, and after a
    // line the template made).
    let mut original_line = false;
    let emit = |out: &mut String, line: &mut bool, space: &str, token: &str, original: bool| {
        if space.contains('\n') {
            *line = original;
        }
        out.push_str(space);
        out.push_str(token);
    };
    for op in ops {
        let (o, n) = (op.old_range(), op.new_range());
        if !matches!(op.tag(), similar::DiffTag::Equal) {
            for (k, (space, token)) in b[n.clone()].iter().enumerate() {
                // A token that replaces others takes their place: the
                // original's whitespace before the first of them (not at
                // the edit's start, which sits where the match did, and
                // not where there was none, so `x+y` → `x in y` keeps the
                // template's spaces). Only where the template separates
                // the tokens too: a token it joins to the one before
                // (`a::b`, `f(x)`) stays joined.
                let in_place = k == 0
                    && n.start != 0
                    && !o.is_empty()
                    && !a[o.start].0.is_empty()
                    && !space.is_empty();
                // A new list element put before one that starts its own
                // line (`f(\n    a,` → `f(extra, a,`) gets a line of its
                // own, indented like it, rather than joining the line
                // above while the rest keep theirs.
                let own_line = k == 0
                    && n.start != 0
                    && o.is_empty()
                    && !space.contains('\n')
                    && b[n.clone()].last().is_some_and(|t| t.1 == ",")
                    && a.get(o.start).is_some_and(|next| {
                        next.0.contains('\n') && !matches!(next.1, "}" | ")" | "]")
                    })
                    && b.get(n.end).is_some_and(|next| !next.0.contains('\n'));
                // Likewise after the last one: what the template appends to
                // a sequence the original lays out one per line (a field
                // after `$$$F`, an item after `$$$B`, just before a closing
                // `}` on its own line) starts a line, indented like the
                // line before, instead of joining that line's end.
                let appended = k == 0
                    && n.start != 0
                    && o.is_empty()
                    && o.start > 0
                    && !space.contains('\n')
                    && a.get(o.start).is_some_and(|next| next.0.contains('\n'))
                    && !matches!(a[o.start - 1].1, "{" | "(" | "[")
                    && !own_line
                    // An element starts with a name or a string, not a
                    // closing `)` the template adds (`e` → `g(e)`).
                    && token.starts_with(|c: char| {
                        c.is_alphanumeric()
                            || matches!(c, '_' | '"' | '\'' | '#' | '@')
                            // A capture laid out as one token.
                            || (SENTINEL..SENTINEL + 0x100).contains(&u32::from(c))
                    })
                    && !b[n.clone()].iter().any(|t| t.0.contains('\n'));
                if appended {
                    let line = &out[out.rfind('\n').map_or(0, |p| p + 1)..];
                    let indent = &line[..line.len() - line.trim_start().len()];
                    let space = format!("\n{indent}");
                    emit(&mut out, &mut original_line, &space, token, true);
                } else if in_place || own_line {
                    emit(&mut out, &mut original_line, a[o.start].0, token, true);
                } else {
                    emit(&mut out, &mut original_line, space, token, false);
                }
            }
            continue;
        }
        let new_start = n.start;
        for (k, ((old_space, token), (new_space, _))) in a[o.clone()].iter().zip(&b[n]).enumerate()
        {
            let (old_break, new_break) = (old_space.contains('\n'), new_space.contains('\n'));
            let next_to_change = k == 0 && (o.start != 0 || new_start != 0);
            let original = if new_start + k == 0 {
                // The edit's first token sits where the match did.
                false
            } else if next_to_change && !old_break {
                // The original's space belonged to a token that is gone,
                // and without a line break nothing of its layout is lost.
                false
            } else if old_break && new_break {
                // A line both break: as the line before it went, so a
                // block keeps its indentation and a re-indented one
                // stays re-indented.
                original_line
            } else {
                // Where only the template breaks the line, its layout
                // stands; elsewhere (a closing brace on its own line, a
                // body a one-line template would join) the original's.
                !new_break
            };
            let space = if original { old_space } else { new_space };
            emit(&mut out, &mut original_line, space, token, original);
        }
    }
    // Trailing whitespace belongs to whichever text has the last word.
    out.push_str(&new[new.trim_end().len()..]);
    out
}

/// `text` as `(whitespace before, token)` pairs: a quoted string, a run
/// of identifier characters, or any other single character.
pub(super) fn layout_tokens(text: &str) -> Vec<(&str, &str)> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut space_start = 0;
    while let Some(&(i, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let space = &text[space_start..i];
        let mut end = i + c.len_utf8();
        chars.next();
        if matches!(c, '"' | '\'' | '`') {
            let mut escaped = false;
            for (j, d) in chars.by_ref() {
                end = j + d.len_utf8();
                if escaped {
                    escaped = false;
                } else if d == '\\' {
                    escaped = true;
                } else if d == c || d == '\n' {
                    break;
                }
            }
        } else if word(c) {
            while let Some(&(j, d)) = chars.peek() {
                if !word(d) {
                    break;
                }
                end = j + d.len_utf8();
                chars.next();
            }
        }
        out.push((space, &text[i..end]));
        space_start = end;
    }
    out
}

/// `source[span]` with every outermost match of any of `forms` (one
/// rewriter's pattern as each context compiled it) rewritten, the text
/// between them kept; of two overlapping edits the outer one is made.
pub(super) fn rewrite_with(
    forms: &[Rewriter<'_, '_>],
    source: &str,
    span: std::ops::Range<usize>,
) -> String {
    let mut edits: Vec<Edit> = forms
        .iter()
        .flat_map(|form| {
            form.outermost(span.clone(), None)
                .into_iter()
                .map(|i| form.render(source, i))
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

/// ast-grep re-indents a multi-line template's later lines to the
/// match's column with spaces, and not at all when the line is indented
/// with tabs. In that case give each later non-empty line the matched
/// line's own indentation, tabs and all.
pub(crate) fn indent_with_tabs(source: &str, at: usize, text: &str) -> String {
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = source[line_start..at]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    if !indent.contains('\t') || !text.contains('\n') {
        return text.to_string();
    }
    let mut lines = text.split_inclusive('\n');
    let mut out = lines.next().unwrap_or_default().to_string();
    for line in lines {
        if !line.trim().is_empty() {
            out.push_str(&indent);
        }
        out.push_str(line);
    }
    out
}

/// ast-grep indents every line of a multi-line template to the match's
/// column, blank ones included, which leaves trailing whitespace. Empty
/// those lines again.
pub(crate) fn strip_blank_line_indent(text: &str) -> String {
    if !text.contains('\n') {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len() - 1;
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            // The first line follows the match's own prefix and the last
            // precedes the rest of its line; only lines in between are
            // whole lines.
            if i != 0 && i != last && line.trim().is_empty() {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` (the rewrite of `node`) with `node`'s leading modifiers put back
/// where the template leaves them out: `kinds` names them (anonymous
/// keywords such as `static`, named kinds such as
/// `accessibility_modifier`), and a pattern matches an item whatever its
/// modifiers (`led_forms`), so `f2() {}` for `private static f() {}` gives
/// `private static f2() {}`. A modifier the pattern spells (in `head`, the
/// pattern before its first `(` or `{`) and the template does not is left
/// out, as the template asks; one the template writes stays where it is.
pub(super) fn keep_modifier_tokens<D: ast_grep_core::Doc>(
    node: &ast_grep_core::Node<'_, D>,
    source: &str,
    text: String,
    keep: Option<(&[&'static str], &str)>,
) -> String {
    let Some((kinds, head)) = keep else {
        return text;
    };
    let mods: Vec<&str> = node
        .children()
        .filter(|c| !c.is_extra())
        .take_while(|c| kinds.contains(&c.kind().as_ref()))
        .map(|c| &source[c.range()])
        .collect();
    if mods.is_empty() {
        return text;
    }
    let spelled = |m: &str| {
        if m == "*" {
            head.contains('*')
        } else {
            head.split_whitespace()
                .any(|w| w.trim_start_matches('*') == m)
        }
    };
    // The template's own leading words, up to the item's name: `get` and
    // `static` in `static get v()`, nothing in `f()`, `x` in `x = 1`.
    // Each leading word with where it starts in `text`.
    let mut have: Vec<(&str, usize)> = Vec::new();
    let mut at = 0;
    loop {
        let start = at + (text[at..].len() - text[at..].trim_start().len());
        let t = &text[start..];
        let end = t.find(char::is_whitespace).unwrap_or(t.len());
        let word = &t[..end];
        if word.is_empty()
            || end == t.len()
            || word.contains(['(', '{', ':', '=', '<', ';', '[', '*'])
        {
            at = start;
            break;
        }
        have.push((word, start));
        at = start + end;
    }
    let mut rest = text[at..].trim_start();
    // In `x = 1` or `x: number`, `x` is the name, not a modifier.
    if rest.starts_with(['=', ':', '?', '!', ';'])
        && let Some((_, start)) = have.pop()
    {
        rest = &text[start..];
    }
    let have: Vec<&str> = have.into_iter().map(|(w, _)| w).collect();
    // A template that writes modifiers of its own, beyond repeating the
    // ones the pattern spells, is used as written.
    if have.iter().any(|w| !spelled(w)) {
        return text;
    }
    let mut words: Vec<&str> = Vec::new();
    for m in &mods {
        let written = have.contains(m) || (*m == "*" && rest.starts_with('*'));
        if have.contains(m) || (!written && !spelled(m)) {
            words.push(m);
        }
    }
    words.extend(have.iter().filter(|w| !mods.contains(w)));
    if words == have {
        return text;
    }
    let mut out = String::with_capacity(text.len() + 16);
    for word in &words {
        out.push_str(word);
        // `*gen()` is written against the name.
        if *word != "*" {
            out.push(' ');
        }
    }
    out.push_str(rest);
    out
}

/// `template` (as [`transformable_template`] writes it, `$NAME`) with each
/// of `empty` (sequence captures that matched nothing) taken out together
/// with the separator that would be left over: one of the spaces around
/// it (`{ log(); $B }` → `{ log(); }`), or its comma (`g($R, 1)` →
/// `g(1)`, `g(1, $R)` → `g(1)`). `None` when nothing changes.
pub(crate) fn tighten(template: &str, empty: &[&str]) -> Option<String> {
    let mut out = template.to_string();
    for name in empty {
        let var = format!("${name}");
        let mut from = 0;
        while let Some(found) = out[from..].find(&var) {
            let at = from + found;
            let end = at + var.len();
            // `$B` in `$BODY` is another name.
            if out[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                from = end;
                continue;
            }
            let (before, after) = (&out[..at], &out[end..]);
            let cut = if after.starts_with(", ") {
                at..end + 2
            } else if after.starts_with(',') {
                at..end + 1
            } else if before.ends_with(", ") {
                at - 2..end
            } else if before.ends_with(' ') && after.starts_with(' ') {
                at..end + 1
            } else {
                at..end
            };
            out.replace_range(cut.clone(), "");
            from = cut.start;
        }
    }
    (out != template).then_some(out)
}

/// `template` with every `$$$NAME` written as `$NAME`, and the names it
/// uses — so ast-grep takes each binding's value from the rewritten text
/// [`Rewriter::render`] supplies instead of from the original source.
pub(crate) fn transformable_template(template: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(template.len());
    let mut names: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let run = tail.chars().take_while(|&c| c == '$').count().min(3);
        let after = &tail[run..];
        let len = after
            .find(|c: char| !(c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()))
            .unwrap_or(after.len());
        let name = &after[..len];
        let is_var = matches!(run, 1 | 3) && after.starts_with(is_var_start);
        if is_var {
            out.push('$');
            out.push_str(name);
            if !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
            rest = &after[len..];
        } else {
            out.push_str(&tail[..run]);
            rest = after;
        }
    }
    out.push_str(rest);
    (out, names)
}

/// The metavariables `pattern` binds, in order of first use. `$_` and a
/// bare `$$$` bind nothing.
pub(super) fn metavariables(pattern: &str) -> Vec<Var> {
    let mut out: Vec<Var> = Vec::new();
    let mut rest = pattern;
    while let Some(i) = rest.find('$') {
        let tail = &rest[i..];
        let run = tail.chars().take_while(|&c| c == '$').count();
        let after = &tail[run..];
        let len = after
            .find(|c: char| !(c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()))
            .unwrap_or(after.len());
        let name = &after[..len];
        if matches!(run, 1 | 3)
            && name != "_"
            && after.starts_with(is_var_start)
            && !out.iter().any(|v| v.name == name)
        {
            out.push(Var {
                name: name.to_string(),
                multi: run == 3,
            });
        }
        rest = &after[len..];
    }
    out
}

/// A template metavariable the pattern does not bind (nor a `let`) would
/// be filled with nothing; refuse it. A `where` pattern's metavariables
/// are its own, so `$F` from `follows '$V, err := $F($$$A)'` is not one.
pub(super) fn unbound_in_template(
    pattern: &str,
    template: &str,
    constraints: &[Constraint],
) -> Result<(), String> {
    let bound = metavariables(pattern);
    let lets: Vec<&str> = constraints
        .iter()
        .filter_map(|c| match &c.test {
            Test::Let { var, .. } => Some(var.name.as_str()),
            _ => None,
        })
        .collect();
    let (_, names) = transformable_template(template);
    let Some(name) = names
        .iter()
        .find(|n| !bound.iter().any(|b| b.name == **n) && !lets.contains(&n.as_str()))
    else {
        return Ok(());
    };
    let mut known: Vec<String> = bound.iter().map(ToString::to_string).collect();
    known.extend(lets.iter().map(|l| format!("${l}")));
    let binds = if known.is_empty() {
        "binds none".to_string()
    } else {
        format!("binds {}", known.join(", "))
    };
    Err(format!(
        "the template uses `${name}`, which the pattern does not bind ({binds}), so it would be \
         filled with nothing; a `where` pattern's metavariables are its own"
    ))
}

impl StructuralPattern {
    /// Replace every match with `template`, substituting `$NAME` /
    /// `$$$NAME` bindings.
    ///
    /// Matches can nest — `errors.Wrap(errors.Wrap(err, "a"), "b")` holds
    /// two. ast-grep's own `replace_all` rewrites only the outermost,
    /// which leaves the inner one for a second run: a rule that is not
    /// idempotent. Here every binding is rewritten first, recursively,
    /// and the template is filled with those rewritten texts, so one pass
    /// reaches the fixed point. Only matches present in `source` are
    /// rewritten; text the template introduces is never re-matched, so a
    /// wrap-style template (`f($A)` → `f(g($A))`) cannot run away.
    pub fn rewrite(&self, source: &str, template: &str) -> String {
        self.rewrite_in(source, template, None)
    }

    /// [`rewrite`](Self::rewrite), judged in `frame` when `source` is a
    /// macro's contents.
    pub(super) fn rewrite_in(
        &self,
        source: &str,
        template: &str,
        frame: Option<&Frame<'_>>,
    ) -> String {
        let forms: Vec<&StructuralPattern> = std::iter::once(self).chain(&self.also).collect();
        self.rewrite_forms(&forms, source.to_string(), template, frame)
    }

    /// Each compilation in `forms` rewrites the text in turn (they match
    /// different nodes), then the macro calls in it are rewritten, which
    /// the text's tree cannot see into. In a macro's contents each stage
    /// is judged in the frame of the text as the stages before left it.
    fn rewrite_forms(
        &self,
        forms: &[&StructuralPattern],
        text: String,
        template: &str,
        frame: Option<&Frame<'_>>,
    ) -> String {
        let Some((form, rest)) = forms.split_first() else {
            return self.edit_macros(&text, frame, |wrapped, inner| {
                self.rewrite_in(wrapped, template, Some(inner))
            });
        };
        let edits = form.rewrite_edits(&text, template, frame);
        if edits.is_empty() {
            return self.rewrite_forms(rest, text, template, frame);
        }
        let mut spans: Vec<(usize, usize, usize)> = edits
            .iter()
            .map(|e| (e.start, e.end, e.text.len()))
            .collect();
        spans.sort_unstable();
        let applied = apply_edits(&text, edits);
        let next = frame.map(|f| f.after(spans, applied.len()));
        self.rewrite_forms(rest, applied, template, next.as_ref())
    }

    /// Where the first match whose rewrite alone leaves `source` (which
    /// parses) unparseable starts, as `(line, column)`, 1-based: the site
    /// a "would not parse" skip is about. `None` when no single rewrite
    /// breaks it (only their combination does), or none is made.
    pub fn first_breaking_rewrite(&self, source: &str, template: &str) -> Option<(usize, usize)> {
        std::iter::once(self).chain(&self.also).find_map(|form| {
            let edit = form
                .rewrite_edits(source, template, None)
                .into_iter()
                .find(|edit| {
                    let one = apply_edits(source, vec![edit.clone()]);
                    parse_clean(&self.lang.grammar, &one).is_err()
                })?;
            Some(Lines::new(source).at(edit.start))
        })
    }

    /// What a rewrite with `template` needs to keep a match's modifiers:
    /// the language's list and the pattern's head (before its first `(` or
    /// `{`). `None` without a list, and for a template that only unwraps
    /// (`$$$B`), which leaves no item to put them on.
    fn kept_modifiers(&self, template: &str) -> Option<(&'static [&'static str], &str)> {
        let tokens = self.lang.modifier_tokens;
        // A template that is one metavariable unwraps the item.
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
        let text = self.text.as_str();
        Some((tokens, &text[..text.find(['(', '{']).unwrap_or(text.len())]))
    }

    /// The edits one compilation makes to `source`, outermost matches only.
    fn rewrite_edits(
        &self,
        source: &str,
        template: &str,
        frame: Option<&Frame<'_>>,
    ) -> Vec<crate::Edit> {
        let grep = parsed(&self.lang, source);
        let matches = self.matches(&grep, source, frame);
        if matches.is_empty() {
            return Vec::new();
        }
        let requalified = self.qualified.as_ref().and_then(|q| q.template(template));
        let template = requalified.as_deref().unwrap_or(template);
        let revisible = self.visible.as_ref().and_then(|v| v.template(template));
        let template = revisible.as_deref().unwrap_or(template);
        let short = self
            .shorthand
            .as_ref()
            .map(|(kind, fields)| shorthand_template(template, fields, *kind));
        let template = short.as_deref().unwrap_or(template);
        let template = if self.loose_semicolon {
            template.trim_end().strip_suffix(';').unwrap_or(template)
        } else {
            template
        };
        let (template, names) = transformable_template(template);
        let fix = TemplateFix::with_transform(&template, &self.lang, &names);
        // A `let`'s `rewrite(<name>)` step: for each form the rewriter's
        // pattern compiled to (its `also` ones too), the fix and its
        // matches in this source; one list per `let`, empty without one.
        type Parts<'g> = Vec<(
            &'g StructuralPattern,
            TemplateFix,
            Vec<NodeMatch<'g, StrDoc<TsLang>>>,
        )>;
        let sub_parts: Vec<Parts<'_>> = self
            .lets
            .iter()
            .map(|d| {
                let grep = &grep;
                d.rewriters
                    .iter()
                    .flat_map(|(sub, template)| {
                        let (template, names) = transformable_template(template);
                        std::iter::once(sub).chain(&sub.also).map(move |form| {
                            let fix = TemplateFix::with_transform(&template, &form.lang, &names);
                            (form, fix, form.matches(grep, source, frame))
                        })
                    })
                    .collect()
            })
            .collect();
        let subs: Vec<Vec<Rewriter<'_, '_>>> = sub_parts
            .iter()
            .map(|parts| {
                parts
                    .iter()
                    .map(|(form, fix, matches)| Rewriter {
                        pattern: &form.pattern,
                        fix,
                        lets: &[],
                        subs: &[],
                        matches,
                        template: None,
                        keep: None,
                        order: match_order(matches),
                    })
                    .collect()
            })
            .collect();
        let rewriter = Rewriter {
            pattern: &self.pattern,
            fix: &fix,
            lets: &self.lets,
            subs: &subs,
            matches: &matches,
            template: Some((&template, &self.lang, &names)),
            keep: self.kept_modifiers(&template),
            order: match_order(&matches),
        };
        let edits: Vec<crate::Edit> = rewriter
            .outermost(0..source.len(), None)
            .into_iter()
            .map(|i| rewriter.render(source, i))
            .filter(|edit| !same_but_whitespace(&source[edit.start..edit.end], &edit.text))
            .collect();
        self.with_attachments(source, &template, frame, edits)
    }

    /// A template that only unwraps the match (nothing but its
    /// metavariables: `mod tests { $$$B }` → `$$$B`) leaves nothing for
    /// the match's doc comments and attributes to belong to, so each edit
    /// takes them, as a delete does ([`Leading`]): the `#[cfg(test)]` goes
    /// with the module it gated. Any other template keeps them above what
    /// it writes, so a rename-style rewrite of an attributed item keeps its
    /// attributes. In a macro's contents there are none to take.
    fn with_attachments(
        &self,
        source: &str,
        template: &str,
        frame: Option<&Frame<'_>>,
        mut edits: Vec<crate::Edit>,
    ) -> Vec<crate::Edit> {
        let leading = self.lang.leading;
        let none = leading.kinds.is_empty() && leading.doc_comments.is_empty();
        if none || frame.is_some() || edits.is_empty() || !only_metavariables(template) {
            return edits;
        }
        let Some(tree) = crate::parse_cache::parse(&self.lang.grammar, source) else {
            return edits;
        };
        for edit in &mut edits {
            let start = super::delete::attached_start(&tree, source, edit.start, edit.end, leading);
            if start < edit.start {
                // The template's text goes where the first attachment was,
                // at that line's indentation.
                edit.start = start;
            }
        }
        edits
    }
}

/// `true` when `template` is metavariables and whitespace only (`$$$B`,
/// `$A $B`): it unwraps what it matched rather than writing new code.
pub(crate) fn only_metavariables(template: &str) -> bool {
    let (_, names) = transformable_template(template);
    if names.is_empty() {
        return false;
    }
    let mut rest = template.to_string();
    for name in &names {
        rest = rest
            .replace(&format!("$$${name}"), "")
            .replace(&format!("${name}"), "");
    }
    rest.trim().is_empty()
}

/// `true` when `a` and `b` differ only in whitespace outside quoted text.
///
/// A multi-line `$$$` capture filled into a one-line template loses the
/// line breaks around it, so `f($$$A)` → `f($$$A)` would reflow every
/// call it matched and report each as changed. Such an edit changes no
/// token, so it is not made. Whitespace inside `"…"`, `'…'` or `` `…` ``
/// is compared as is (a string's spaces are its content); a quote the
/// scan misreads, such as a Rust lifetime, only keeps more whitespace in
/// the comparison, so the edit is made, as it would be without the check.
pub(crate) fn same_but_whitespace(a: &str, b: &str) -> bool {
    fn tokens(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut quote: Option<char> = None;
        let mut escaped = false;
        for c in text.chars() {
            match quote {
                Some(q) => {
                    out.push(c);
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == q {
                        quote = None;
                    }
                }
                None if c.is_whitespace() => {}
                None => {
                    if matches!(c, '"' | '\'' | '`') {
                        quote = Some(c);
                    }
                    out.push(c);
                }
            }
        }
        out
    }
    a != b && tokens(a) == tokens(b)
}
