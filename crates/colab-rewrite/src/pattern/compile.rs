//! The text transforms behind a pattern's extra forms
//! (`crate::syntax::compile_forms`): a bare call written qualified or
//! rooted, and fields written as the shorthand.

use super::*;

pub(super) fn is_var_start(c: char) -> bool {
    c.is_ascii_uppercase() || c == '_'
}

/// Words that can stand before `(` without being a callee.
const NOT_CALLEES: &[&str] = &[
    "as", "async", "await", "box", "break", "const", "crate", "dyn", "else", "fn", "for", "if",
    "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
    "Self", "static", "super", "type", "unsafe", "use", "where", "while", "yield",
];

/// When `text` starts with a bare call (`f(`, or `f::<T>(` with `sep` =
/// `::`) or a struct literal of a type named bare (`FileChange {`: an
/// upper-case name, so a keyword's block never counts), the same text with
/// [`QUALIFIER`] and `sep` before it, and the callee's (or type's) name.
/// `None` for anything else: a metavariable callee, a path that is already
/// qualified, a macro, a keyword.
pub(crate) fn qualify_call<'t>(text: &'t str, sep: &str) -> Option<(String, &'t str)> {
    let body = text.trim_start();
    let lead = &text[..text.len() - body.len()];
    let end = body
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(body.len());
    let callee = &body[..end];
    let first = callee.chars().next()?;
    if !(first.is_alphabetic() || first == '_') || callee == "_" || NOT_CALLEES.contains(&callee) {
        return None;
    }
    let rest = body[end..].trim_start();
    let turbofish = rest
        .strip_prefix(sep)
        .is_some_and(|r| r.trim_start().starts_with('<'));
    let literal = rest.starts_with('{') && first.is_uppercase();
    (rest.starts_with('(') || turbofish || literal)
        .then(|| (format!("{lead}{QUALIFIER}{sep}{body}"), callee))
}

/// The fields of `text` written `name: $VAR` in a field-list place: right
/// after an opening bracket of `shorthand` or `,` and right before `,` or
/// a closing one (whitespace aside), with `$VAR` a single metavariable the
/// pattern names nowhere else. So `let x: $T = …` and (with braces only)
/// `fn f(a: $T)` never count, nor does a Ruby `def`'s `(a: $T)`, whose
/// shorthand `a:` is a required keyword, not one set from `a`. Each is
/// `(the bytes from the name to the end of $VAR, name, VAR)`.
pub(crate) fn shorthand_fields(
    text: &str,
    shorthand: Shorthand,
) -> Vec<(std::ops::Range<usize>, String, String)> {
    let definition = text.trim_start().starts_with("def ");
    let bytes = text.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // A name starts after a `{` or `,` (skipping whitespace).
        if !(bytes[i].is_ascii_lowercase() || bytes[i] == b'_') || (i > 0 && ident(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let before = text[..i].trim_end();
        let start = i;
        while i < bytes.len() && ident(bytes[i]) {
            i += 1;
        }
        let name = &text[start..i];
        let opened = before.ends_with(shorthand.open) && !(definition && before.ends_with('('));
        if !(opened || before.ends_with(',')) {
            continue;
        }
        let Some(value) = text[i..].trim_start().strip_prefix(':') else {
            continue;
        };
        if value.starts_with(':') {
            continue; // a path, `a::b`
        }
        let Some(var) = value.trim_start().strip_prefix('$') else {
            continue;
        };
        if var.starts_with('$') {
            continue; // `$$$` takes many nodes
        }
        let len = var
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(var.len());
        let var_name = &var[..len];
        let end = text.len() - var.len() + len;
        let next = text[end..].trim_start();
        let once = text.matches(&format!("${var_name}")).count() == 1;
        if !var_name.is_empty()
            && once
            && (next.starts_with(',') || next.starts_with(shorthand.close))
            && !(definition && next.starts_with(')'))
        {
            out.push((start..end, name.to_string(), var_name.to_string()));
        }
        i = end;
    }
    out
}

/// A template for a match of a shorthand form (see
/// [`shorthand_fields`]): each `$VAR` is the field's name, and a field
/// that ends up `name: name` is written as the shorthand (`name`, Ruby's
/// `name:`).
pub(crate) fn shorthand_template(
    template: &str,
    fields: &[(String, String)],
    shorthand: Shorthand,
) -> String {
    let mut out = template.to_string();
    for (name, var) in fields {
        let token = format!("${var}");
        let mut at = 0;
        while let Some(found) = out[at..].find(&token) {
            let start = at + found;
            let end = start + token.len();
            let longer = out[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if longer || out[..start].ends_with('$') {
                at = end;
                continue;
            }
            out.replace_range(start..end, name);
            at = start + name.len();
        }
        out = out.replace(
            &format!("{name}: {name}"),
            &format!("{name}{}", shorthand.kept),
        );
    }
    out
}

/// `qualified` (from [`qualify_call`]) with `sep` before its path, after
/// any leading whitespace: `$COLAB_PATH::f()` → `::$COLAB_PATH::f()`.
pub(crate) fn root_path(qualified: &str, sep: &str) -> String {
    let body = qualified.trim_start();
    format!("{}{sep}{body}", &qualified[..qualified.len() - body.len()])
}
