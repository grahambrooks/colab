//! Rewrite Java call expressions using a template.
//!
//! `method_invocation` carries an optional `object` field, so the callee
//! is read from the source span between the start of the call and its
//! argument list. Targets read exactly as the call is written:
//!
//! | Source | Target |
//! | ------ | ------ |
//! | `helper(x)` | `"helper"` |
//! | `Old.run(a)` | `"Old.run"` |
//! | `this.foo(y)` | `"this.foo"` |
//!
//! Qualified and bare forms are therefore distinct targets and never
//! match each other.
//!
//! Idempotency: templates that keep the callee text match their own
//! output and re-apply on every pass. Rename the method to stay
//! re-runnable.

use std::fmt;
use std::path::Path;

use colab_core::{Operation, render_call_template};
use tree_sitter::{Node, TreeCursor};

const CALL_KINDS: &[&str] = &["method_invocation"];

#[derive(Debug)]
pub struct CallReplace {
    pub function: String,
    pub template: String,
}

impl fmt::Display for CallReplace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "java::call \"{}\" -> replace_call \"{}\"",
            self.function, self.template
        )
    }
}

impl Operation for CallReplace {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("java")
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
        // raw source keeps the receiver in the target, which is what makes
        // `Old.run` and a bare `run` distinguishable.
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
    fn renames_a_qualified_call() {
        let src = "class A { void m() { Old.run(a, b); } }\n";
        let out = rewrite("Old.run", "New.run($args)", src);
        assert_eq!(out, "class A { void m() { New.run(a, b); } }\n");
    }

    #[test]
    fn renames_a_bare_call() {
        let src = "class A { void m() { helper(x); } }\n";
        let out = rewrite("helper", "newHelper($args)", src);
        assert_eq!(out, "class A { void m() { newHelper(x); } }\n");
    }

    #[test]
    fn a_bare_name_does_not_match_a_qualified_call() {
        let src = "class A { void m() { Old.run(a); } }\n";
        assert_eq!(rewrite("run", "WRONG($args)", src), src);
    }

    #[test]
    fn matches_a_this_qualified_call() {
        let src = "class A { void m() { this.foo(y); } }\n";
        let out = rewrite("this.foo", "this.bar($args)", src);
        assert_eq!(out, "class A { void m() { this.bar(y); } }\n");
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "class A { void m() { Old.run(a, b); } }\n";
        let out = rewrite("Old.run", "New.run($2, $1, null)", src);
        assert_eq!(out, "class A { void m() { New.run(b, a, null); } }\n");
    }

    #[test]
    fn handles_zero_args() {
        let src = "class A { void m() { x(); } }\n";
        assert_eq!(rewrite("x", "y($args)", src), "class A { void m() { y(); } }\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_the_method_is_renamed() {
        let src = "class A { void m() { Old.run(a); } }\n";
        let once = rewrite("Old.run", "New.run($args)", src);
        assert_eq!(rewrite("Old.run", "New.run($args)", &once), once);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "class A { void m() { other(1); } }\n";
        assert_eq!(rewrite("Old.run", "New.run($args)", src), src);
    }
}
