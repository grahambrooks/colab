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
use std::borrow::Cow;

use tree_sitter::Node;

#[cfg(test)]
fn rewrite(function: &str, template: &str, source_code: &str) -> String {
    crate::LANG.rewrite_calls(function, template, source_code)
}

/// `receiver.method` when there is a receiver, else `method`.
fn callee_text(node: Node<'_>, source: &str) -> Option<String> {
    let method = node.child_by_field_name("method")?;
    let method_text = method.utf8_text(source.as_bytes()).ok()?;
    match node.child_by_field_name("receiver") {
        Some(recv) => {
            let recv_text = recv.utf8_text(source.as_bytes()).ok()?;
            Some(format!("{recv_text}.{method_text}"))
        }
        None => Some(method_text.to_string()),
    }
}

/// The argument list and callee of a parenthesised `call` node.
///
/// Both `f(a)` and `f a` produce an `argument_list`; only the
/// parenthesised form starts with `(`. Rewriting the bare form could
/// change how the surrounding expression parses, so it is skipped (see
/// module docs).
pub(crate) fn paren_call<'a>(node: Node<'a>, source: &'a str) -> Option<(Node<'a>, Cow<'a, str>)> {
    if node.kind() != "call" {
        return None;
    }
    let args = node.child_by_field_name("arguments")?;
    if !source[args.start_byte()..].starts_with('(') {
        return None;
    }
    Some((args, callee_text(node, source)?.into()))
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
