//! The call rewrite behind `<lang>::call "<function>" { replace_call "<template>" }`
//! for the backends whose calls are one node kind with `function` and
//! `arguments` fields (C, C++, C#, Go, JS, Python, Rust).
//!
//! Matching is exact equality on the `function` field's source text, so
//! `old_fn` and `ns_old_fn` are distinct. The template's placeholders are
//! [`colab_core::render_call_template`]'s.

use colab_core::render_call_template;
use tree_sitter::{Node, Tree};

/// `source` with every `call_kind` node calling `function` replaced by
/// `template`, filled with that call's arguments.
pub fn rewrite(
    tree: &Tree,
    source: &str,
    call_kind: &str,
    function: &str,
    template: &str,
) -> String {
    let mut edits = Vec::new();
    crate::visit_all(tree, |node| {
        if let Some(edit) = call_edit(node, source, call_kind, function, template) {
            edits.push(edit);
        }
    });
    crate::apply_edits(source, edits)
}

fn call_edit(
    node: Node<'_>,
    source: &str,
    call_kind: &str,
    function: &str,
    template: &str,
) -> Option<crate::Edit> {
    if node.kind() != call_kind {
        return None;
    }
    let callee = node.child_by_field_name("function")?;
    if callee.utf8_text(source.as_bytes()).ok()? != function {
        return None;
    }
    let args = node.child_by_field_name("arguments")?;
    let texts = arguments(args, source);
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    Some(crate::Edit::new(
        node.start_byte(),
        node.end_byte(),
        render_call_template(template, function, &refs),
    ))
}

/// [`rewrite`] for a language whose calls are not one node kind with
/// `function` / `arguments` fields (Java, Kotlin, PHP, Ruby, Swift):
/// `locate` reads each call's argument list and callee its own way.
/// With `through_arguments`, the edit ends at the argument list, so a
/// trailing lambda or closure after it survives.
pub fn rewrite_located(
    tree: &Tree,
    source: &str,
    locate: crate::language::CallLocator,
    through_arguments: bool,
    function: &str,
    template: &str,
) -> String {
    let mut edits = Vec::new();
    crate::visit_all(tree, |node| {
        if let Some((args, callee)) = locate(node, source)
            && callee == function
        {
            let texts = arguments(args, source);
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            let end = if through_arguments {
                args.end_byte()
            } else {
                node.end_byte()
            };
            edits.push(crate::Edit::new(
                node.start_byte(),
                end,
                render_call_template(template, function, &refs),
            ));
        }
    });
    crate::apply_edits(source, edits)
}

/// Every callee `locate` finds in `tree`: the near-miss candidates of a
/// [`rewrite_located`] rule.
pub fn callees_located(
    tree: &Tree,
    source: &str,
    locate: crate::language::CallLocator,
) -> Vec<String> {
    let mut out = Vec::new();
    crate::visit_all(tree, |node| {
        if let Some((_, callee)) = locate(node, source) {
            out.push(callee.into_owned());
        }
    });
    out
}

/// The argument list's named children, as written. (C# wraps each in an
/// `argument` node, which may carry `ref` / `out`: its text is what
/// belongs in `$1` / `$args`.)
fn arguments(list: Node<'_>, source: &str) -> Vec<String> {
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .filter_map(|child| child.utf8_text(source.as_bytes()).ok().map(str::to_string))
        .collect()
}
