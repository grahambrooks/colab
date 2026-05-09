//! Rewrite Rust call expressions using a template.
//!
//! Mirrors `colab-lang-go::calls`. Tree-sitter-rust represents
//! function calls as `call_expression` with `function` and
//! `arguments` fields; we match on the `function` node's source text
//! exactly. Method calls like `x.foo()` are *method_call_expression*
//! nodes — those are excluded so a rule targeting `foo` does not
//! collide with a method named `foo`.

use std::fmt;
use std::path::Path;

use colab_core::{Operation, render_call_template};
use tree_sitter::{Node, Parser, TreeCursor};

#[derive(Debug)]
pub struct CallReplace {
    pub function: String,
    pub template: String,
}

impl fmt::Display for CallReplace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rust::call \"{}\" -> replace_call \"{}\"",
            self.function, self.template
        )
    }
}

impl Operation for CallReplace {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("rs")
    }

    fn apply(&self, source_code: &str) -> String {
        rewrite(&self.function, &self.template, source_code)
    }
}

pub fn rewrite(function: &str, template: &str, source_code: &str) -> String {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("failed to load tree-sitter Rust grammar");
    let Some(tree) = parser.parse(source_code, None) else {
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
    fn renames_a_function_call_with_args_passthrough() {
        let src = "fn main() { pkg::old(a, b); }\n";
        let out = rewrite("pkg::old", "pkg::new($args)", src);
        assert_eq!(out, "fn main() { pkg::new(a, b); }\n");
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "fn main() { pkg::old(a, b); }\n";
        let out = rewrite("pkg::old", "pkg::new($2, $1, None)", src);
        assert_eq!(out, "fn main() { pkg::new(b, a, None); }\n");
    }

    #[test]
    fn ignores_method_calls() {
        // `x.foo()` is a method_call_expression, not a call_expression
        // whose function field equals "foo".
        let src = "fn main() { x.foo(1); }\n";
        assert_eq!(rewrite("foo", "bar($args)", src), src);
    }

    #[test]
    fn handles_zero_args() {
        let src = "fn main() { trace(); }\n";
        let out = rewrite("trace", "log($args)", src);
        assert_eq!(out, "fn main() { log(); }\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_function_renamed() {
        let src = "fn main() { pkg::old(a); }\n";
        let once = rewrite("pkg::old", "pkg::new($args)", src);
        let twice = rewrite("pkg::old", "pkg::new($args)", &once);
        assert_eq!(once, twice);
    }
}
