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
        let inside: Vec<(usize, std::ops::Range<usize>)> = self
            .matches
            .iter()
            .enumerate()
            .filter(|&(i, _)| Some(i) != outer)
            .map(|(i, m)| (i, m.range()))
            .filter(|(_, r)| r.start >= span.start && r.end <= span.end)
            .collect();
        let mut out: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
        for (i, r) in &inside {
            let covered = inside.iter().any(|(j, other)| {
                let contains = other.start <= r.start && r.end <= other.end;
                contains && (other != r || j < i)
            });
            if !covered {
                out.push((*i, r.clone()));
            }
        }
        out.sort_by_key(|(_, r)| r.start);
        out.into_iter().map(|(i, _)| i).collect()
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
        for (k, derived) in self.lets.iter().enumerate() {
            let Some((_, var, text, span)) = values.iter().find(|(n, ..)| *n == derived.from)
            else {
                continue;
            };
            // A rewriter starts from the binding's source: each of its
            // matches there rewritten, the text between them kept.
            let start = match (self.subs.get(k), span) {
                (Some(forms), Some(span)) if !forms.is_empty() => {
                    rewrite_with(forms, source, span.clone())
                }
                _ => text.clone(),
            };
            let (var, text) = (var.clone(), derived.apply(&start));
            nm.get_env_mut()
                .insert_transformation(&var, &derived.var, text.clone().into_bytes());
            values.push((derived.var.clone(), var, text, None));
        }
        let edit = nm.make_edit(self.pattern, self.fix);
        let text = String::from_utf8_lossy(&edit.inserted_text);
        let text = indent_with_tabs(source, edit.position, &text);
        let (start, end) = (edit.position, edit.position + edit.deleted_length);
        let text = keep_layout(&source[start..end], &strip_blank_line_indent(&text));
        Edit::new(start, end, text)
    }
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
pub(super) fn keep_layout(old: &str, new: &str) -> String {
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
                if in_place {
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
pub(super) fn indent_with_tabs(source: &str, at: usize, text: &str) -> String {
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
pub(super) fn strip_blank_line_indent(text: &str) -> String {
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

/// `template` with every `$$$NAME` written as `$NAME`, and the names it
/// uses — so ast-grep takes each binding's value from the rewritten text
/// [`Rewriter::render`] supplies instead of from the original source.
pub(super) fn transformable_template(template: &str) -> (String, Vec<String>) {
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
