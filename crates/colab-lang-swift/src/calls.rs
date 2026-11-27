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
//!
//! The callee is everything before the argument list (`call_suffix` ›
//! `value_arguments`), trailing whitespace trimmed, and the call is
//! replaced through the argument list, so a trailing closure after it
//! survives (`CallShape::Through`).

#[cfg(test)]
fn rewrite(function: &str, template: &str, source_code: &str) -> String {
    crate::LANG.rewrite_calls(function, template, source_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_a_bare_call() {
        let src = "func f() { oldFn(a, b) }\n";
        assert_eq!(
            rewrite("oldFn", "newFn($args)", src),
            "func f() { newFn(a, b) }\n"
        );
    }

    #[test]
    fn renames_a_navigation_call() {
        let src = "func f() { Old.run(a) }\n";
        assert_eq!(
            rewrite("Old.run", "New.run($args)", src),
            "func f() { New.run(a) }\n"
        );
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
