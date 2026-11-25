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
use colab_syntax::{Node, Tree};

use crate::pattern::{PATTERN_SLOT, PatternLanguage};

/// The next named sibling of `node`, trivia passed over.
fn next_named(node: Node<'_>) -> Option<Node<'_>> {
    std::iter::successors(node.next_sibling(), Node::next_sibling).find(Node::is_named)
}

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
) -> Result<Vec<Generalized>, String> {
    let (before, after) = (before.trim(), after.trim());
    if before == after {
        return Err("`before` and `after` are the same: there is no change to generalize".into());
    }
    // An Edit's text is a slice of lines that need not be one node
    // (`func a() {` and the line changed): the lines both share at either
    // end are context, not the change.
    let (trimmed_b, trimmed_a) = changed_lines(before, after);
    // Unless the changed lines alone are no code (one argument of a call
    // spread over lines, `"Sunday",`): then the whole snippets.
    let (before, after) = match (locate(language, trimmed_b), locate(language, trimmed_a)) {
        (Some(_), Some(_)) => (trimmed_b, trimmed_a),
        _ if locate(language, before).is_some() && locate(language, after).is_some() => {
            (before, after)
        }
        _ => (trimmed_b, trimmed_a),
    };
    let (Some((tree_b, src_b, range_b)), Some((tree_a, src_a, range_a))) =
        (locate(language, before), locate(language, after))
    else {
        // Too little to walk: try the snippets as one unit.
        return generalize_one(language, before, after).map(|g| vec![g]);
    };
    let items_b = items(tree_b.root(), &range_b);
    let items_a = items(tree_a.root(), &range_a);
    if items_b.len() != items_a.len() || items_b.is_empty() {
        return generalize_one(language, before, after).map(|g| vec![g]);
    }
    let mut units: Vec<(Node<'_>, Node<'_>)> = Vec::new();
    for (b, a) in items_b.iter().zip(&items_a) {
        differing(*b, *a, &src_b, &src_a, &mut units);
    }
    // Each unit generalized; one that finds nothing to vary is lifted to
    // its parent, inside the snippet, and tried again.
    let within = |n: Node<'_>, r: &Range<usize>| r.start <= n.start() && n.end() <= r.end;
    let mut rules: Vec<(Range<usize>, Generalized)> = Vec::new();
    let mut last_error = None;
    for (mut b, mut a) in units {
        loop {
            match generalize_one(language, &src_b[b.range()], &src_a[a.range()]) {
                Ok(g) => {
                    rules.push((b.range(), g));
                    break;
                }
                Err(e) => {
                    last_error = Some(e);
                    match (b.parent(), a.parent()) {
                        (Some(pb), Some(pa)) if within(pb, &range_b) && within(pa, &range_a) => {
                            // An argument list stands for its call, as in
                            // `unit`: alone, `(path, "utf8")` parses as a
                            // tuple and the rule would lose its callee.
                            (b, a) = unit(pb, pa);
                            if !within(b, &range_b) || !within(a, &range_a) {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
            }
        }
    }
    // A unit inside another (a lifted callee and its call) is the outer's.
    let ranges: Vec<Range<usize>> = rules.iter().map(|(r, _)| r.clone()).collect();
    let mut out: Vec<Generalized> = Vec::new();
    for (range, g) in rules {
        let inside_another = ranges
            .iter()
            .any(|r| *r != range && r.start <= range.start && range.end <= r.end);
        if !inside_another && !out.contains(&g) {
            out.push(g);
        }
    }
    if out.is_empty() {
        return Err(last_error.unwrap_or_else(|| "no change to generalize".into()));
    }
    Ok(out)
}

/// `before` and `after` without the whole lines they share at the start and
/// at the end, trimmed; unchanged when that would leave either empty.
fn changed_lines<'s>(before: &'s str, after: &'s str) -> (&'s str, &'s str) {
    let lines_b: Vec<&str> = before.lines().collect();
    let lines_a: Vec<&str> = after.lines().collect();
    let shortest = lines_b.len().min(lines_a.len());
    let head = lines_b
        .iter()
        .zip(&lines_a)
        .take_while(|(b, a)| b == a)
        .count()
        .min(shortest);
    let tail = lines_b
        .iter()
        .rev()
        .zip(lines_a.iter().rev())
        .take_while(|(b, a)| b == a)
        .count()
        .min(shortest - head);
    if head + tail >= lines_b.len() || head + tail >= lines_a.len() {
        return (before, after);
    }
    let middle = |lines: &[&str]| lines[head..lines.len() - tail].join("\n");
    let (b, a) = (middle(&lines_b), middle(&lines_a));
    // Borrowed back from the inputs, so the result lives as long as they do.
    match (before.find(b.trim()), after.find(a.trim())) {
        (Some(i), Some(j)) => (
            &before[i..i + b.trim().len()],
            &after[j..j + a.trim().len()],
        ),
        _ => (before, after),
    }
}

/// The nodes a snippet is: the one named node spanning it, or the named
/// children of the node containing it that lie inside it (statements in a
/// block, items in a file).
fn items<'t>(root: Node<'t>, range: &Range<usize>) -> Vec<Node<'t>> {
    let Some(node) = node_spanning(root, range) else {
        return Vec::new();
    };
    if node.range() == *range {
        return vec![node];
    }
    node.named_children()
        .filter(|c| range.start <= c.start() && c.end() <= range.end && !c.is_extra())
        .collect()
}

/// Whether `node` is a list of others (an argument or parameter list): never
/// a unit or a value on its own, only what holds them.
fn is_list(node: &Node<'_>) -> bool {
    let kind = node.kind_name();
    kind.ends_with("arguments")
        || kind.ends_with("argument_list")
        || kind.ends_with("parameters")
        || kind.ends_with("_list")
        || kind == "arguments"
        // Swift: a call's `call_suffix` holds its `value_arguments`.
        || kind == "call_suffix"
}

/// `(b, a)` as a unit: a list stands for the node it belongs to (an
/// argument list for its call), and so does a callee (`x.sort_by` is half
/// of `x.sort_by(…)`'s change); anything inside an import is the import
/// (`std::path::Path` alone would match every type path).
fn unit<'t>(mut b: Node<'t>, mut a: Node<'t>) -> (Node<'t>, Node<'t>) {
    while let (Some(pb), Some(pa)) = (b.parent(), a.parent())
        && (is_list(&b) || field_of(&b) == Some("function"))
    {
        (b, a) = (pb, pa);
    }
    // Inside an import declaration: the whole declaration.
    let mut up = b;
    let mut up_a = a;
    while let (Some(pb), Some(pa)) = (up.parent(), up_a.parent()) {
        if is_import(&pb) {
            return (pb, pa);
        }
        (up, up_a) = (pb, pa);
    }
    (b, a)
}

/// The field `node` sits in under its parent, if any.
fn field_of<'t>(node: &Node<'t>) -> Option<&'t str> {
    node.field_name()
}

/// An import declaration: Rust's `use`, Go's / Java's / JS's `import`, C's
/// `#include`, PHP's `use`.
fn is_import(node: &Node<'_>) -> bool {
    let kind = node.kind_name();
    kind == "use_declaration"
        || kind.starts_with("import")
        || kind.contains("include")
        || kind == "namespace_use_declaration"
        || kind == "using_directive"
}

/// The smallest pairs of subtrees where `b` and `a` differ: descend while
/// both are the same kind with as many named children, stop where not. A
/// differing leaf (a renamed callee) stands for its parent, which holds
/// what varies around it.
fn differing<'t>(
    b: Node<'t>,
    a: Node<'t>,
    src_b: &str,
    src_a: &str,
    out: &mut Vec<(Node<'t>, Node<'t>)>,
) {
    if src_b[b.range()] == src_a[a.range()] {
        return;
    }
    let (nb, na) = (b.named_children().count(), a.named_children().count());
    if b.kind_name() == a.kind_name() && nb == na && nb > 0 {
        let kids_b: Vec<Node<'t>> = b.named_children().collect();
        let kids_a: Vec<Node<'t>> = a.named_children().collect();
        let before = out.len();
        for (kb, ka) in kids_b.iter().zip(&kids_a) {
            if kb.named_children().count() == 0 && src_b[kb.range()] != src_a[ka.range()] {
                // A leaf changed: the unit is this node, around it.
                out.truncate(before);
                out.push(unit(b, a));
                return;
            }
            differing(*kb, *ka, src_b, src_a, out);
        }
        // Only anonymous tokens differ (an operator): this node is the unit.
        if out.len() == before {
            out.push(unit(b, a));
        }
        return;
    }
    out.push(unit(b, a));
}

/// One unit: `before` as one node, generalized against `after`.
fn generalize_one(
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
    let Some(node) = node_spanning(tree.root(), &range) else {
        return Err(
            "`before` is not one syntax node; give one expression, statement or item".into(),
        );
    };

    // The subtrees to abstract, by range in `source`.
    let mut cut: Vec<Range<usize>> = Vec::new();
    if node.range() != range {
        return Err(
            "`before` is not one syntax node; give one expression, statement or item".into(),
        );
    }
    for child in node.children() {
        collect(child, child.field_name(), &node, &source, after, &mut cut);
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
    if field.is_some_and(|f| NAME_FIELDS.contains(&f)) || unnamed_callee(&node, field, parent) {
        return;
    }
    let text = &source[node.range()];
    // A string's inside is not a node a metavariable can stand for (`"$A"`
    // is a literal), and neither is an escape in one (`\n`).
    let kind = node.kind_name();
    if kind.ends_with("content") || kind.ends_with("fragment") || kind.contains("escape") {
        return;
    }
    // A list is what holds the values, not one: look inside it. Text with
    // no letter or digit (`.`) is not carried over by search: as a token it
    // would match every `.` in the template. (As an argument, `""` is a
    // value all the same.)
    let shared = !is_list(&node)
        && text.chars().any(char::is_alphanumeric)
        && !token_positions(after, text).is_empty();
    let argument = !is_list(&node) && parent.kind_name().contains("argument");
    if shared || argument {
        cut.push(node.range());
        return;
    }
    // A string is one value: its quotes (Python's named `string_start`)
    // and pieces are not values of their own to carry over.
    if node.kind_name().contains("string") {
        return;
    }
    for child in node.children() {
        collect(child, child.field_name(), &node, source, after, cut);
    }
}

/// A callee in a grammar that gives it no `function` field (Kotlin's and
/// Swift's `call_expression`): a child of a call, in no field, before the
/// call's arguments. Kept literal like a named one, or `Random(42)` would
/// generalize to `$A($B)`, every call there is.
fn unnamed_callee(node: &Node<'_>, field: Option<&str>, parent: &Node<'_>) -> bool {
    let kind = parent.kind_name();
    if field.is_some() || !(kind.contains("call") || kind.contains("invocation")) {
        return false;
    }
    let mut sibling = next_named(*node);
    while let Some(s) = sibling {
        if is_list(&s) {
            return true;
        }
        sibling = next_named(s);
    }
    false
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
    std::iter::once(PATTERN_SLOT)
        .chain(language.contexts.iter().copied())
        .find_map(|context| {
            let start = context.find(PATTERN_SLOT)?;
            let source = context.replacen(PATTERN_SLOT, text, 1);
            let tree = crate::syntax::fragment_tree(language, &source)?;
            (!tree.has_error()).then(|| (tree, source, start..start + text.len()))
        })
}

/// The outermost named node spanning exactly `range`, else the innermost
/// one containing it.
fn node_spanning<'t>(root: Node<'t>, range: &Range<usize>) -> Option<Node<'t>> {
    let mut node = root;
    loop {
        if node.range() == *range && node.is_named() {
            return Some(node);
        }
        let inner = node
            .named_children()
            .find(|c| c.start() <= range.start && range.end <= c.end());
        match inner {
            Some(child) => node = child,
            None => {
                return (node.range() != root.range() || root.range() == *range).then_some(node);
            }
        }
    }
}
