//! Rewrite Ruby call expressions using a template.
//!
//! Ruby models every call as a `call` node with an optional `receiver`
//! and a `method`. Targets are written the way the call reads:
//!
//! | Source | Target |
//! | ------ | ------ |
//! | `old_fn(a)` | `"old_fn"` |
//! | `Old.run(a)` | `"Old.run"` |
//! | `obj.run(a)` | `"obj.run"` |
//!
//! **Parenthesised calls only.** A call written without parentheses
//! (`puts x`) still parses as a `call`, but rewriting it with a template
//! that adds parentheses would change how the surrounding expression
//! parses in ways colab cannot verify. Those are skipped.
//!
//! Idempotency: a template that keeps the callee text matches its own
//! output and re-applies on every pass. Rename the method to stay
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
            "ruby::call \"{}\" -> replace_call \"{}\"",
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

/// `receiver.method` when there is a receiver, else `method`.
fn callee_text(node: Node<'_>, source: &str) -> Option<String> {
    let method = node.child_by_field_name("method")?;
    let method_text = method.utf8_text(source.as_bytes()).ok()?;
    match node.child_by_field_name("receiver") {
        Some(recv) => {
            let recv_text = recv.utf8_text(source.as_bytes()).ok()?;
            Some(format!("{}.{}", recv_text, method_text))
        }
        None => Some(method_text.to_string()),
    }
}

fn collect(
    node: Node<'_>,
    source: &str,
    function: &str,
    template: &str,
    edits: &mut Vec<(usize, usize, String)>,
) {
    if node.kind() == "call"
        && let Some(args) = node.child_by_field_name("arguments")
        // Both `f(a)` and `f a` produce an `argument_list`; only the
        // parenthesised form starts with `(`. Rewriting the bare form
        // could change how the surrounding expression parses, so it is
        // skipped (see module docs).
        && source[args.start_byte()..].starts_with('(')
        && let Some(callee) = callee_text(node, source)
        && callee == function
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
    fn renames_a_bare_method_call() {
        let src = "old_fn(a, b)\n";
        assert_eq!(rewrite("old_fn", "new_fn($args)", src), "new_fn(a, b)\n");
    }

    #[test]
    fn renames_a_receiver_call() {
        let src = "Old.run(a)\n";
        assert_eq!(rewrite("Old.run", "New.run($args)", src), "New.run(a)\n");
    }

    #[test]
    fn a_bare_name_does_not_match_a_receiver_call() {
        let src = "Old.run(a)\n";
        assert_eq!(rewrite("run", "WRONG($args)", src), src);
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "Old.run(a, b)\n";
        let out = rewrite("Old.run", "New.run($2, $1, nil)", src);
        assert_eq!(out, "New.run(b, a, nil)\n");
    }

    #[test]
    fn paren_less_calls_are_skipped() {
        // Rewriting `puts x` into `log(x)` could change how the
        // surrounding expression parses, so it is not attempted.
        let src = "puts x\n";
        assert_eq!(rewrite("puts", "log($args)", src), src);
    }

    #[test]
    fn handles_zero_args() {
        let src = "x()\n";
        assert_eq!(rewrite("x", "y($args)", src), "y()\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_the_method_is_renamed() {
        let src = "Old.run(a)\n";
        let once = rewrite("Old.run", "New.run($args)", src);
        let twice = rewrite("Old.run", "New.run($args)", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "other(1)\n";
        assert_eq!(rewrite("old_fn", "new_fn($args)", src), src);
    }
}
