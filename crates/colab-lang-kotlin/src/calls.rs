//! Rewrite Kotlin call expressions using a template.
//!
//! A `call_expression` in this grammar has no `function` field: the
//! callee is simply the first child, either an `identifier`
//! (`oldFn(a)`) or a `navigation_expression` (`Thing.run(a)`). The
//! callee text is taken verbatim, so `Thing.run` and `run` are distinct
//! targets.
//!
//! **Trailing-lambda calls are skipped.** `list.map { it }` has no
//! `value_arguments` node, and a template cannot express a lambda body,
//! so rewriting it would lose code.
//!
//! Idempotency: templates that keep the callee text match their own
//! output. Rename the function to stay re-runnable.

use std::fmt;
use std::path::Path;

use colab_core::{Operation, render_call_template};
use tree_sitter::{Node, TreeCursor};

#[derive(Debug)]
pub struct CallReplace {
    pub function: String,
    pub template: String,
}

impl fmt::Display for CallReplace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "kotlin::call \"{}\" -> replace_call \"{}\"",
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
    let mut cursor = tree.walk();
    collect(&mut cursor, source_code, function, template, &mut edits);

    if edits.is_empty() {
        return source_code.to_string();
    }
    edits.sort_by_key(|e| e.0);
    let mut out = source_code.to_string();
    for (start, end, replacement) in edits.iter().rev() {
        out.replace_range(*start..*end, replacement);
    }
    out
}

/// The `value_arguments` child of a call, if it has one.
fn value_arguments<'a>(node: Node<'a>) -> Option<Node<'a>> {
    (0..node.named_child_count())
        .filter_map(|i| node.named_child(i as u32))
        .find(|c| c.kind() == "value_arguments")
}

fn collect(
    cursor: &mut TreeCursor,
    source: &str,
    function: &str,
    template: &str,
    edits: &mut Vec<(usize, usize, String)>,
) {
    let node = cursor.node();
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
        // lambda after the parens survives untouched.
        edits.push((node.start_byte(), args.end_byte(), rendered));
    }

    if cursor.goto_first_child() {
        loop {
            collect(cursor, source, function, template, edits);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
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
        let src = "fun main() { oldFn(a, b) }\n";
        let out = rewrite("oldFn", "newFn($args)", src);
        assert_eq!(out, "fun main() { newFn(a, b) }\n");
    }

    #[test]
    fn renames_a_navigation_call() {
        let src = "fun main() { Thing.run(a) }\n";
        let out = rewrite("Thing.run", "Thing.execute($args)", src);
        assert_eq!(out, "fun main() { Thing.execute(a) }\n");
    }

    #[test]
    fn a_bare_name_does_not_match_a_navigation_call() {
        let src = "fun main() { Thing.run(a) }\n";
        assert_eq!(rewrite("run", "WRONG($args)", src), src);
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "fun main() { Thing.run(a, b) }\n";
        let out = rewrite("Thing.run", "Thing.run2($2, $1, null)", src);
        assert_eq!(out, "fun main() { Thing.run2(b, a, null) }\n");
    }

    #[test]
    fn handles_zero_args() {
        let src = "fun main() { x() }\n";
        assert_eq!(rewrite("x", "y($args)", src), "fun main() { y() }\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_the_function_is_renamed() {
        let src = "fun main() { Thing.run(a) }\n";
        let once = rewrite("Thing.run", "Thing.execute($args)", src);
        let twice = rewrite("Thing.run", "Thing.execute($args)", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "fun main() { other(1) }\n";
        assert_eq!(rewrite("oldFn", "newFn($args)", src), src);
    }
}
