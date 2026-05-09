//! Rewrite Go call expressions using a template.
//!
//! `match go::call "pkg.Old" { replace_call "pkg.New($2, $1, nil)" }`
//! finds every `pkg.Old(...)` call and rewrites it according to the
//! template — see [`colab_core::render_call_template`] for the
//! placeholder language.
//!
//! Function matching is exact-equality on the `function` field's
//! source text, so `pkg.Old`, `Old`, and `(*T).Old` are distinct.
//!
//! Idempotency rule: if the rendered template still produces a call
//! expression whose function text equals the matched name, the rule
//! will fire again on the next pass. Templates **must rename the
//! function** (e.g. `pkg.Old → pkg.New`) for safe re-runs. The
//! corpus harness's idempotency check fails fast if a rule violates
//! this. Wrap-style transforms (e.g. inserting a leading argument
//! while keeping the function name) are intentionally not
//! idempotent and must be applied with a single `--write` pass.

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
            "go::call \"{}\" -> replace_call \"{}\"",
            self.function, self.template
        )
    }
}

impl Operation for CallReplace {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("go")
    }

    fn apply(&self, source_code: &str) -> String {
        rewrite(&self.function, &self.template, source_code)
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
        let Some(child) = arg_list.named_child(i) else {
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
    fn renames_call_and_keeps_args() {
        let src = "package main\nfunc main() { pkg.Old(a, b) }\n";
        let out = rewrite("pkg.Old", "pkg.New($args)", src);
        assert_eq!(out, "package main\nfunc main() { pkg.New(a, b) }\n");
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "package main\nfunc main() { pkg.Old(a, b) }\n";
        let out = rewrite("pkg.Old", "pkg.New($2, $1, nil)", src);
        assert_eq!(out, "package main\nfunc main() { pkg.New(b, a, nil) }\n");
    }

    #[test]
    fn preserves_call_arguments_with_complex_expressions() {
        let src = "package main\nfunc main() { do(f(x), 1+2) }\n";
        let out = rewrite("do", "doNew($args)", src);
        assert_eq!(out, "package main\nfunc main() { doNew(f(x), 1+2) }\n");
    }

    #[test]
    fn does_not_match_unrelated_calls() {
        let src = "package main\nfunc main() { other(1) }\n";
        assert_eq!(rewrite("pkg.Old", "pkg.New($args)", src), src);
    }

    #[test]
    fn does_not_match_substring_of_function_name() {
        // `Old` (bare) must not match `pkg.Old` (selector).
        let src = "package main\nfunc main() { pkg.Old(1) }\n";
        assert_eq!(rewrite("Old", "WrongOld($args)", src), src);
    }

    #[test]
    fn rewrite_is_idempotent_when_function_renamed() {
        let src = "package main\nfunc main() { pkg.Old(a, b) }\n";
        let once = rewrite("pkg.Old", "pkg.New($args)", src);
        let twice = rewrite("pkg.Old", "pkg.New($args)", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn handles_zero_args() {
        let src = "package main\nfunc main() { x() }\n";
        let out = rewrite("x", "y($args)", src);
        assert_eq!(out, "package main\nfunc main() { y() }\n");
    }

    #[test]
    fn returns_input_unchanged_when_target_absent() {
        let src = "package main\nfunc main() {}\n";
        assert_eq!(rewrite("missing.Func", "other.Func($args)", src), src);
    }
}
