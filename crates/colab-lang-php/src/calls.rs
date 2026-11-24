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
use std::borrow::Cow;

use tree_sitter::Node;

const CALL_KINDS: &[&str] = &[
    "function_call_expression",
    "scoped_call_expression",
    "member_call_expression",
    "nullsafe_member_call_expression",
];

#[cfg(test)]
fn rewrite(function: &str, template: &str, source_code: &str) -> String {
    crate::LANG.rewrite_calls(function, template, source_code)
}

/// The argument list of a call node and its callee: the raw source
/// before the argument list, trailing whitespace trimmed. Taking the
/// callee as raw source keeps the receiver in the target, which is what
/// makes qualified and bare calls distinguishable.
pub(crate) fn callee<'a>(node: Node<'a>, source: &'a str) -> Option<(Node<'a>, Cow<'a, str>)> {
    if !CALL_KINDS.contains(&node.kind()) {
        return None;
    }
    let args = node.child_by_field_name("arguments")?;
    let callee = source.get(node.start_byte()..args.start_byte())?;
    Some((args, callee.trim_end().into()))
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
