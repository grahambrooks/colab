//! Rewrite JavaScript/TypeScript call expressions using a template.
//!
//! `match js::call "mod.old" { replace_call "..." }` finds every
//! matching call and rewrites it — see [`colab_core::render_call_template`]
//! for the placeholder language.
//!
//! Matching is exact equality on the callee's source text, so
//! `mod.old` and a bare method name are distinct targets.
//!
//! Idempotency: a template that keeps the callee text matches its own
//! output and re-applies on every pass. Rename the function to stay
//! re-runnable.

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
            "js::call \"{}\" -> replace_call \"{}\"",
            self.function, self.template
        )
    }
}

impl Operation for CallReplace {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::imports::is_relevant(path)
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
    fn renames_a_plain_call() {
        let src = "oldFn(a, b);\n";
        assert_eq!(rewrite("oldFn", "newFn($args)", src), "newFn(a, b);\n");
    }

    #[test]
    fn renames_a_member_call() {
        let src = "mod.old(x);\n";
        assert_eq!(rewrite("mod.old", "mod.new($args)", src), "mod.new(x);\n");
    }

    #[test]
    fn a_bare_name_does_not_match_a_member_call() {
        let src = "mod.old(x);\n";
        assert_eq!(rewrite("old", "WRONG($args)", src), src);
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "oldFn(a, b);\n";
        assert_eq!(
            rewrite("oldFn", "newFn($2, $1, null)", src),
            "newFn(b, a, null);\n"
        );
    }

    #[test]
    fn works_on_typescript_sources_too() {
        let src = "const x: number = oldFn(1);\n";
        assert_eq!(
            rewrite("oldFn", "newFn($args)", src),
            "const x: number = newFn(1);\n"
        );
    }

    #[test]
    fn handles_zero_args() {
        assert_eq!(rewrite("x", "y($args)", "x();\n"), "y();\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_the_function_is_renamed() {
        let src = "mod.old(a);\n";
        let once = rewrite("mod.old", "mod.new($args)", src);
        assert_eq!(rewrite("mod.old", "mod.new($args)", &once), once);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "other(1);\n";
        assert_eq!(rewrite("oldFn", "newFn($args)", src), src);
    }
}
