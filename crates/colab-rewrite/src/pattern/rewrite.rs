//! The text work of rewriting a match, shared with `crate::syntax`'s
//! rewriter: keeping the matched code's layout and indentation, putting
//! back modifier keywords, tightening a template around an empty
//! sequence, and checking a template's metavariables.

use super::*;

/// First of the private-use characters a capture is laid out as, one per
/// capture ([`keep_layout`] treats one as a token of its own).
pub(crate) const SENTINEL: u32 = 0xF0000;

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

/// A multi-line template's later non-empty lines, each given the matched
/// line's own indentation, spaces or tabs, so the template's own
/// indentation is relative to the match's line.
pub(crate) fn indent_later_lines(source: &str, at: usize, text: &str) -> String {
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = source[line_start..at]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    if indent.is_empty() || !text.contains('\n') {
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

/// A blank line of a multi-line template is left empty, not indented, so
/// a rewrite leaves no trailing whitespace.
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

/// [`keep_modifier_tokens`]'s text work, for either engine: `text` with the
/// item's leading modifiers `mods` (their texts) put back where the
/// template leaves them out, given `head`, the pattern before its first
/// `(` or `{`.
pub(crate) fn keep_modifier_words(mods: &[&str], text: String, head: &str) -> String {
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
    for m in mods {
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
/// uses: the rewriter fills each name with its binding's rewritten text.
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
pub(crate) fn unbound_in_template(
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
