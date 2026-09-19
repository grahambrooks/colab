//! Rewrite C++ call expressions using a template.
//!
//! `match cpp::call "old_fn" { replace_call "new_fn($args)" }` finds every
//! `old_fn(...)` and rewrites it — see [`colab_core::render_call_template`]
//! for the placeholder language.
//!
//! Matching is exact equality on the `function` field's source text, so
//! `old_fn` and `ns_old_fn` are distinct.
//!
//! Idempotency: a template that keeps the function name (e.g.
//! `old_fn → old_fn(ctx, $args)`) matches its own output and will wrap
//! again on the next pass. Templates **must rename the function** to be
//! safe to re-run; the corpus harness fails any rule that loops.

use std::fmt;
use std::path::Path;

use colab_core::{Operation, render_call_template};
use tree_sitter::Node;

#[derive(Debug)]
pub struct CallReplace {
    pub function: String,
    pub template: String,
}

impl fmt::Display for CallReplace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cpp::call \"{}\" -> replace_call \"{}\"",
            self.function, self.template
        )
    }
}

impl Operation for CallReplace {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        rewrite(&self.function, &self.template, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.function)
    }
}

pub fn rewrite(function: &str, template: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    colab_rewrite::visit_all(&tree, |node| {
        collect(node, source_code, function, template, &mut edits)
    });

    colab_rewrite::apply_edits(
        source_code,
        edits
            .into_iter()
            .map(|(start, end, text)| colab_rewrite::Edit::new(start, end, text))
            .collect(),
    )
}

fn collect(
    node: Node<'_>,
    source: &str,
    function: &str,
    template: &str,
    edits: &mut Vec<(usize, usize, String)>,
) {
    if node.kind() == "call_expression"
        && let Some(func_node) = node.child_by_field_name("function")
        && let Ok(func_text) = func_node.utf8_text(source.as_bytes())
        && func_text == function
        && let Some(args) = node.child_by_field_name("arguments")
    {
        let arg_strs = collect_args(args, source);
        let arg_refs: Vec<&str> = arg_strs.iter().map(|s| s.as_str()).collect();
        let rendered = render_call_template(template, function, &arg_refs);
        edits.push((node.start_byte(), node.end_byte(), rendered));
    }
}

fn collect_args<'a>(arg_list: Node<'a>, source: &'a str) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..arg_list.named_child_count() {
        let Some(child) = arg_list.named_child(i as u32) else {
            break;
        };
        if let Ok(text) = child.utf8_text(source.as_bytes()) {
            out.push(text.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_a_call_and_passes_args_through() {
        let src = "void f(void) { old_fn(a, b); }\n";
        let out = rewrite("old_fn", "new_fn($args)", src);
        assert_eq!(out, "void f(void) { new_fn(a, b); }\n");
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "void f(void) { old_fn(a, b); }\n";
        let out = rewrite("old_fn", "new_fn($2, $1, NULL)", src);
        assert_eq!(out, "void f(void) { new_fn(b, a, NULL); }\n");
    }

    #[test]
    fn does_not_match_a_substring_of_a_longer_name() {
        let src = "void f(void) { ns_old_fn(1); }\n";
        assert_eq!(rewrite("old_fn", "WRONG($args)", src), src);
    }

    #[test]
    fn handles_zero_args() {
        let src = "void f(void) { x(); }\n";
        assert_eq!(rewrite("x", "y($args)", src), "void f(void) { y(); }\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_the_function_is_renamed() {
        let src = "void f(void) { old_fn(a); }\n";
        let once = rewrite("old_fn", "new_fn($args)", src);
        let twice = rewrite("old_fn", "new_fn($args)", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "void f(void) { other(1); }\n";
        assert_eq!(rewrite("old_fn", "new_fn($args)", src), src);
    }
}
