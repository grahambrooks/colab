//! One example edit generalized into a rule (`colab.generalize`): the
//! agent has made the change once by hand; colab infers the pattern and
//! template that make it everywhere else.
//!
//! A light anti-unification over `before`'s syntax tree:
//!
//! - a subtree whose text `after` also holds, as whole tokens, varies from
//!   site to site and is carried over: it becomes a metavariable in both
//!   (`err` in `errors.Wrap(err, …)` → `fmt.Errorf(…, err)`);
//! - an argument the edit changed or dropped becomes one too, used in the
//!   pattern only (`foo(a, b)` → `foo(a)` is `foo($A, $B)` → `foo($A)`);
//! - names stay literal: a callee, a field, a type, a key (whatever sits
//!   in a `function` / `name` / `field` / … field of its parent), so the
//!   rule does not widen to every call;
//! - everything else is the change itself, written as it is.
//!
//! When `after` holds a captured value inside a longer token (a string
//! that quotes it), the template cannot derive it; a warning says so.

use std::ops::Range;

use colab_core::Generalized;
use tree_sitter::{Node, Parser, Tree};

use crate::pattern::{PATTERN_SLOT, PatternLanguage};

/// Fields whose node names something: kept literal, and not looked into.
const NAME_FIELDS: &[&str] = &[
    "function",
    "name",
    "field",
    "property",
    "attribute",
    "method",
    "type",
    "macro",
    "constructor",
    "key",
    "label",
];

/// The rule that turns `before` into `after` wherever else it applies.
pub fn generalize(
    language: &PatternLanguage,
    before: &str,
    after: &str,
) -> Result<Generalized, String> {
    let (before, after) = (before.trim(), after.trim());
    if before == after {
        return Err("`before` and `after` are the same: there is no change to generalize".into());
    }
    if before.is_empty() {
        return Err("`before` is empty: give the code as it was".into());
    }
    let (tree, source, range) = locate(language, before).ok_or_else(|| {
        "`before` does not parse as code in this language, alone or in any of its pattern \
         contexts; give a whole expression, statement or item"
            .to_string()
    })?;
    let Some(node) = node_spanning(tree.root_node(), &range) else {
        return Err(
            "`before` is not one syntax node; give one expression, statement or item".into(),
        );
    };

    // The subtrees to abstract, by range in `source`.
    let mut cut: Vec<Range<usize>> = Vec::new();
    let mut cursor = node.walk();
    for (i, child) in node.children(&mut cursor).enumerate() {
        let field = node.field_name_for_child(u32::try_from(i).unwrap_or(u32::MAX));
        collect(child, field, &node, &source, after, &mut cut);
    }

    // One metavariable per distinct text, in order.
    let mut bindings: Vec<(String, String)> = Vec::new();
    let mut named: Vec<(Range<usize>, String)> = Vec::new();
    for range in &cut {
        let text = source[range.clone()].to_string();
        let var = match bindings.iter().find(|(_, t)| *t == text) {
            Some((var, _)) => var.clone(),
            None => {
                let var = format!("${}", var_name(bindings.len()));
                bindings.push((var.clone(), text));
                var
            }
        };
        named.push((range.clone(), var));
    }
    if bindings.is_empty() {
        return Err(
            "nothing in `before` varies or is carried into `after`, so the rule would match only \
             this exact code; write the rule by hand, or give an edit with an argument or \
             expression in it"
                .into(),
        );
    }

    let mut pattern = source[range.clone()].to_string();
    for (cut, var) in named.iter().rev() {
        let (start, end) = (cut.start - range.start, cut.end - range.start);
        pattern.replace_range(start..end, var);
    }
    let template = fill_template(after, &bindings);

    let mut warnings = Vec::new();
    for (var, text) in &bindings {
        if !token_positions(after, text).is_empty() {
            continue;
        }
        // A string's contents count too: `"read config"` quoted inside
        // `"read config: %w"`.
        let inner = text.trim_matches(['"', '\'', '`']);
        if after.contains(text.as_str()) || (inner.len() > 1 && after.contains(inner)) {
            warnings.push(format!(
                "`{var}` (`{text}` here) appears in `after` only inside a longer token, so the \
                 template writes that text as it is at every match; edit the template (a `let` \
                 step can derive text from `{var}`) if it should vary"
            ));
        }
    }
    Ok(Generalized {
        pattern,
        template,
        bindings,
        warnings,
    })
}

/// Walk `node` (the field it is in, and its parent), adding the ranges to
/// abstract.
fn collect(
    node: Node<'_>,
    field: Option<&str>,
    parent: &Node<'_>,
    source: &str,
    after: &str,
    cut: &mut Vec<Range<usize>>,
) {
    if !node.is_named() || node.is_extra() {
        return;
    }
    if field.is_some_and(|f| NAME_FIELDS.contains(&f)) {
        return;
    }
    let text = &source[node.byte_range()];
    let shared = !text.trim().is_empty() && !token_positions(after, text).is_empty();
    let argument = parent.kind().contains("argument");
    if shared || argument {
        cut.push(node.byte_range());
        return;
    }
    let mut cursor = node.walk();
    for (i, child) in node.children(&mut cursor).enumerate() {
        let field = node.field_name_for_child(u32::try_from(i).unwrap_or(u32::MAX));
        collect(child, field, &node, source, after, cut);
    }
}

/// `$A`, `$B`, … `$Z`, `$A1`, …
fn var_name(index: usize) -> String {
    let letter = char::from(b'A' + u8::try_from(index % 26).unwrap_or(0));
    match index / 26 {
        0 => letter.to_string(),
        round => format!("{letter}{round}"),
    }
}

/// Where `needle` occurs in `text` as whole tokens: not inside a longer
/// identifier or number on either side.
fn token_positions(text: &str, needle: &str) -> Vec<usize> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let starts_word = needle.chars().next().is_some_and(word);
    let ends_word = needle.chars().next_back().is_some_and(word);
    text.match_indices(needle)
        .filter(|(at, _)| {
            let before_ok = !starts_word || !text[..*at].chars().next_back().is_some_and(word);
            let end = at + needle.len();
            let after_ok = !ends_word || !text[end..].chars().next().is_some_and(word);
            before_ok && after_ok
        })
        .map(|(at, _)| at)
        .collect()
}

/// `after` with each binding's text, where it stands as whole tokens,
/// written as its metavariable (longest texts first, never overlapping).
fn fill_template(after: &str, bindings: &[(String, String)]) -> String {
    let mut by_length: Vec<&(String, String)> = bindings.iter().collect();
    by_length.sort_by_key(|(_, text)| std::cmp::Reverse(text.len()));
    let mut spans: Vec<(Range<usize>, &str)> = Vec::new();
    for (var, text) in by_length {
        for at in token_positions(after, text) {
            let span = at..at + text.len();
            if spans
                .iter()
                .all(|(s, _)| span.end <= s.start || s.end <= span.start)
            {
                spans.push((span, var));
            }
        }
    }
    spans.sort_by_key(|(s, _)| s.start);
    let mut out = String::with_capacity(after.len());
    let mut at = 0;
    for (span, var) in spans {
        out.push_str(&after[at..span.start]);
        out.push_str(var);
        at = span.end;
    }
    out.push_str(&after[at..]);
    out
}

/// `text` parsed on its own or in the first pattern context that reads it
/// without errors: the tree, the source parsed, and where `text` is in it.
fn locate(language: &PatternLanguage, text: &str) -> Option<(Tree, String, Range<usize>)> {
    let mut parser = Parser::new();
    parser.set_language(&language.grammar).ok()?;
    std::iter::once(PATTERN_SLOT)
        .chain(language.contexts.iter().copied())
        .find_map(|context| {
            let start = context.find(PATTERN_SLOT)?;
            let source = context.replacen(PATTERN_SLOT, text, 1);
            let tree = parser.parse(&source, None)?;
            (!tree.root_node().has_error()).then(|| (tree, source, start..start + text.len()))
        })
}

/// The outermost named node spanning exactly `range`, else the innermost
/// one containing it.
fn node_spanning<'t>(root: Node<'t>, range: &Range<usize>) -> Option<Node<'t>> {
    let mut node = root;
    loop {
        if node.byte_range() == *range && node.is_named() {
            return Some(node);
        }
        let mut cursor = node.walk();
        let inner = node
            .named_children(&mut cursor)
            .find(|c| c.start_byte() <= range.start && range.end <= c.end_byte());
        match inner {
            Some(child) => node = child,
            None => {
                return (node.byte_range() != root.byte_range() || root.byte_range() == *range)
                    .then_some(node);
            }
        }
    }
}
