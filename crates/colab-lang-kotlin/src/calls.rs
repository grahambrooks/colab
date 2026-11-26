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

//!
//! The callee is everything before the argument list, trailing whitespace
//! trimmed, and the call is replaced through the argument list, so a
//! trailing lambda after it survives (`CallShape::Through`).

#[cfg(test)]
fn rewrite(function: &str, template: &str, source_code: &str) -> String {
    crate::LANG.rewrite_calls(function, template, source_code)
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
