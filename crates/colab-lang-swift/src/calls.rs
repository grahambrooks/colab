//! Rewrite Swift call expressions using a template.
//!
//! A `call_expression` has no `function` field: the callee is the first
//! child, either a `simple_identifier` (`oldFn(a)`) or a
//! `navigation_expression` (`Old.run(a)`). The callee text is taken
//! verbatim, so `Old.run` and `run` are distinct targets.
//!
//! Arguments sit inside a `call_suffix`. **Trailing-closure calls are
//! skipped** — a template cannot express a closure body, so rewriting one
//! would lose code.
//!
//! Argument labels are part of the argument text, so `$1` for
//! `f(name: x)` expands to `name: x`. Reordering labelled arguments with
//! positional placeholders therefore keeps each label with its value.
//!
//! Idempotency: templates that keep the callee text match their own
//! output. Rename the function to stay re-runnable.

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
            "swift::call \"{}\" -> replace_call \"{}\"",
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
    colab_rewrite::visit_all(&tree, |node| collect(node, source_code, function, template, &mut edits));

    colab_rewrite::apply_edits(
        source_code,
        edits
            .into_iter()
            .map(|(start, end, text)| colab_rewrite::Edit::new(start, end, text))
            .collect(),
    )
}

/// The `value_arguments` node of a call, which Swift nests inside a
/// `call_suffix`.
fn value_arguments<'a>(node: Node<'a>) -> Option<Node<'a>> {
    let suffix = (0..node.named_child_count())
        .filter_map(|i| node.named_child(i as u32))
        .find(|c| c.kind() == "call_suffix")?;
    (0..suffix.named_child_count())
        .filter_map(|i| suffix.named_child(i as u32))
        .find(|c| c.kind() == "value_arguments")
}

fn collect(
    node: Node<'_>,
    source: &str,
    function: &str,
    template: &str,
    edits: &mut Vec<(usize, usize, String)>,
) {
    if node.kind() == "call_expression"
        && let Some(args) = value_arguments(node)
        // The callee is everything before the argument list.
        && let Some(callee) = source.get(node.start_byte()..args.start_byte())
        && callee.trim_end() == function
    {
        let arg_strs = collect_args(args, source);
        let arg_refs: Vec<&str> = arg_strs.iter().map(|s| s.as_str()).collect();
        let rendered = render_call_template(template, function, &arg_refs);
        // Replace only up to the end of the argument list so a trailing
        // closure after the parens survives untouched.
        edits.push((node.start_byte(), args.end_byte(), rendered));
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
    fn renames_a_bare_call() {
        let src = "func f() { oldFn(a, b) }\n";
        assert_eq!(rewrite("oldFn", "newFn($args)", src), "func f() { newFn(a, b) }\n");
    }

    #[test]
    fn renames_a_navigation_call() {
        let src = "func f() { Old.run(a) }\n";
        assert_eq!(rewrite("Old.run", "New.run($args)", src), "func f() { New.run(a) }\n");
    }

    #[test]
    fn a_bare_name_does_not_match_a_navigation_call() {
        let src = "func f() { Old.run(a) }\n";
        assert_eq!(rewrite("run", "WRONG($args)", src), src);
    }

    #[test]
    fn argument_labels_travel_with_their_values() {
        let src = "func f() { Old.run(first: a, second: b) }\n";
        let out = rewrite("Old.run", "New.run($2, $1)", src);
        assert_eq!(out, "func f() { New.run(second: b, first: a) }\n");
    }

    #[test]
    fn handles_zero_args() {
        let src = "func f() { x() }\n";
        assert_eq!(rewrite("x", "y($args)", src), "func f() { y() }\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_the_function_is_renamed() {
        let src = "func f() { Old.run(a) }\n";
        let once = rewrite("Old.run", "New.run($args)", src);
        let twice = rewrite("Old.run", "New.run($args)", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "func f() { other(1) }\n";
        assert_eq!(rewrite("oldFn", "newFn($args)", src), src);
    }
}
