//! Rewrite C# call expressions using a template.
//!
//! `match csharp::call "Foo.Old" { replace_call "new_fn($args)" }` finds every
//! `Foo.Old(...)` and rewrites it — see [`colab_core::render_call_template`]
//! for the placeholder language.
//!
//! Matching is exact equality on the `function` field's source text, so
//! `Foo.Old`, `Old`, and `this.Old` are distinct.
//!
//! Idempotency: a template that keeps the function name (e.g.
//! `old_fn → old_fn(ctx, $args)`) matches its own output and will wrap
//! again on the next pass. Templates **must rename the function** to be
//! safe to re-run; the corpus harness fails any rule that loops.

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
            "csharp::call \"{}\" -> replace_call \"{}\"",
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
    if node.kind() == "invocation_expression"
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
        let Some(child) = arg_list.named_child(i as u32) else {
            break;
        };
        // C# wraps each argument in an `argument` node (which may carry
        // a `ref`/`out` modifier); its text is what belongs in $1/$args.
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
        let src = "class C { void M() { Foo.Old(a, b); } }\n";
        let out = rewrite("Foo.Old", "Foo.New($args)", src);
        assert_eq!(out, "class C { void M() { Foo.New(a, b); } }\n");
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "class C { void M() { Foo.Old(a, b); } }\n";
        let out = rewrite("Foo.Old", "Foo.New($2, $1, null)", src);
        assert_eq!(out, "class C { void M() { Foo.New(b, a, null); } }\n");
    }

    #[test]
    fn does_not_match_a_bare_name_against_a_qualified_call() {
        let src = "class C { void M() { Foo.Old(1); } }\n";
        assert_eq!(rewrite("Old", "WRONG($args)", src), src);
    }

    #[test]
    fn handles_zero_args() {
        let src = "class C { void M() { X(); } }\n";
        assert_eq!(rewrite("X", "Y($args)", src), "class C { void M() { Y(); } }\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_the_function_is_renamed() {
        let src = "class C { void M() { Foo.Old(a); } }\n";
        let once = rewrite("Foo.Old", "Foo.New($args)", src);
        let twice = rewrite("Foo.Old", "Foo.New($args)", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "class C { void M() { Other(1); } }\n";
        assert_eq!(rewrite("Foo.Old", "Foo.New($args)", src), src);
    }
}
