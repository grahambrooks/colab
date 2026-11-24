//! Rewrite C++ call expressions using a template.
//!
//! `match cpp::call "old_fn" { replace_call "new_fn($args)" }` finds every
//! `old_fn(...)` and rewrites it — see [`colab_core::render_call_template`]
//! for the placeholder language.
//!
//! Matching is exact equality on the `function` field's source text, so
//! `old_fn` and `ns_old_fn` are distinct.
//!
//! Idempotency: a template that keeps the function name (e.g.
//! `old_fn → old_fn(ctx, $args)`) matches its own output and will wrap
//! again on the next pass. Templates **must rename the function** to be
//! safe to re-run; the corpus harness fails any rule that loops.

#[cfg(test)]
fn rewrite(function: &str, template: &str, source_code: &str) -> String {
    crate::LANG.rewrite_calls(function, template, source_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_a_call_and_passes_args_through() {
        let src = "void f(void) { old_fn(a, b); }\n";
        let out = rewrite("old_fn", "new_fn($args)", src);
        assert_eq!(out, "void f(void) { new_fn(a, b); }\n");
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "void f(void) { old_fn(a, b); }\n";
        let out = rewrite("old_fn", "new_fn($2, $1, NULL)", src);
        assert_eq!(out, "void f(void) { new_fn(b, a, NULL); }\n");
    }

    #[test]
    fn does_not_match_a_substring_of_a_longer_name() {
        let src = "void f(void) { ns_old_fn(1); }\n";
        assert_eq!(rewrite("old_fn", "WRONG($args)", src), src);
    }

    #[test]
    fn handles_zero_args() {
        let src = "void f(void) { x(); }\n";
        assert_eq!(rewrite("x", "y($args)", src), "void f(void) { y(); }\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_the_function_is_renamed() {
        let src = "void f(void) { old_fn(a); }\n";
        let once = rewrite("old_fn", "new_fn($args)", src);
        let twice = rewrite("old_fn", "new_fn($args)", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "void f(void) { other(1); }\n";
        assert_eq!(rewrite("old_fn", "new_fn($args)", src), src);
    }
}
