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

/// The argument list's named children, as written. (C# wraps each in an
/// `argument` node, which may carry `ref` / `out`: its text is what
/// belongs in `$1` / `$args`.)
fn arguments(list: Node<'_>, source: &str) -> Vec<String> {
    let mut cursor = list.walk();
    list.named_children(&mut cursor)
        .filter_map(|child| child.utf8_text(source.as_bytes()).ok().map(str::to_string))
        .collect()
}
