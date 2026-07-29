//! Rewrite PHP call expressions using a template.
//!
//! PHP has three call node kinds, all matched on the callee text exactly
//! as written:
//!
//! | Source | Target |
//! | ------ | ------ |
//! | `helper($a)` | `"helper"` |
//! | `Old::run($a)` | `"Old::run"` |
//! | `$obj->run($a)` | `"$obj->run"` |
//!
//! The callee text is taken verbatim from the source between the start of
//! the call and the start of its argument list, so the `::` and `->`
//! forms are distinct targets and never match each other.
//!
//! Idempotency: templates that keep the callee text match their own
//! output and will re-apply on every pass. Rename the function to stay
//! re-runnable.

use std::fmt;
use std::path::Path;

use colab_core::{Operation, render_call_template};
use tree_sitter::{Node, TreeCursor};

const CALL_KINDS: &[&str] = &[
    "function_call_expression",
    "scoped_call_expression",
    "member_call_expression",
    "nullsafe_member_call_expression",
];

#[derive(Debug)]
pub struct CallReplace {
    pub function: String,
    pub template: String,
}

impl fmt::Display for CallReplace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "php::call \"{}\" -> replace_call \"{}\"",
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

fn collect(
    cursor: &mut TreeCursor,
    source: &str,
    function: &str,
    template: &str,
    edits: &mut Vec<(usize, usize, String)>,
) {
    let node = cursor.node();
    if CALL_KINDS.contains(&node.kind())
        && let Some(args) = node.child_by_field_name("arguments")
        // The callee is everything before the argument list. Taking it as
        // raw source keeps `::` and `->` in the target, which is what
        // makes those forms distinguishable.
        && let Some(callee) = source.get(node.start_byte()..args.start_byte())
        && callee.trim_end() == function
    {
        let arg_strs = collect_args(args, source);
        let arg_refs: Vec<&str> = arg_strs.iter().map(|s| s.as_str()).collect();
        let rendered = render_call_template(template, function, &arg_refs);
        edits.push((node.start_byte(), node.end_byte(), rendered));
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
    fn renames_a_plain_function_call() {
        let src = "<?php\nhelper($a, $b);\n";
        let out = rewrite("helper", "newHelper($args)", src);
        assert_eq!(out, "<?php\nnewHelper($a, $b);\n");
    }

    #[test]
    fn renames_a_static_call() {
        let src = "<?php\nOld::run($a);\n";
        let out = rewrite("Old::run", "New::run($args)", src);
        assert_eq!(out, "<?php\nNew::run($a);\n");
    }

    #[test]
    fn renames_a_member_call() {
        let src = "<?php\n$obj->run($a);\n";
        let out = rewrite("$obj->run", "$obj->execute($args)", src);
        assert_eq!(out, "<?php\n$obj->execute($a);\n");
    }

    #[test]
    fn static_and_member_forms_are_distinct_targets() {
        let src = "<?php\nOld::run($a);\n";
        // A bare name must not match a static call.
        assert_eq!(rewrite("run", "WRONG($args)", src), src);
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "<?php\nOld::run($a, $b);\n";
        let out = rewrite("Old::run", "New::run($2, $1, null)", src);
        assert_eq!(out, "<?php\nNew::run($b, $a, null);\n");
    }

    #[test]
    fn handles_zero_args() {
        let src = "<?php\nx();\n";
        assert_eq!(rewrite("x", "y($args)", src), "<?php\ny();\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_the_function_is_renamed() {
        let src = "<?php\nOld::run($a);\n";
        let once = rewrite("Old::run", "New::run($args)", src);
        let twice = rewrite("Old::run", "New::run($args)", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "<?php\nother($a);\n";
        assert_eq!(rewrite("helper", "newHelper($args)", src), src);
    }
}
