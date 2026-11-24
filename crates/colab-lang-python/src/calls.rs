//! Rewrite Python call expressions using a template.
//!
//! `match python::call "mod.old" { replace_call "..." }` finds every
//! matching call and rewrites it — see [`colab_core::render_call_template`]
//! for the placeholder language.
//!
//! Matching is exact equality on the callee's source text, so
//! `mod.old` and a bare method name are distinct targets.
//!
//! Idempotency: a template that keeps the callee text matches its own
//! output and re-applies on every pass. Rename the function to stay
//! re-runnable.

#[cfg(test)]
fn rewrite(function: &str, template: &str, source_code: &str) -> String {
    crate::LANG.rewrite_calls(function, template, source_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_a_plain_call() {
        let src = "old_fn(a, b)\n";
        assert_eq!(rewrite("old_fn", "new_fn($args)", src), "new_fn(a, b)\n");
    }

    #[test]
    fn renames_an_attribute_call() {
        let src = "mod.old(x)\n";
        assert_eq!(rewrite("mod.old", "mod.new($args)", src), "mod.new(x)\n");
    }

    #[test]
    fn a_bare_name_does_not_match_an_attribute_call() {
        let src = "mod.old(x)\n";
        assert_eq!(rewrite("old", "WRONG($args)", src), src);
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "old_fn(a, b)\n";
        assert_eq!(
            rewrite("old_fn", "new_fn($2, $1, None)", src),
            "new_fn(b, a, None)\n"
        );
    }

    #[test]
    fn keyword_arguments_travel_with_their_names() {
        let src = "old_fn(x=1, y=2)\n";
        assert_eq!(
            rewrite("old_fn", "new_fn($2, $1)", src),
            "new_fn(y=2, x=1)\n"
        );
    }

    #[test]
    fn handles_zero_args() {
        assert_eq!(rewrite("x", "y($args)", "x()\n"), "y()\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_the_function_is_renamed() {
        let src = "mod.old(a)\n";
        let once = rewrite("mod.old", "mod.new($args)", src);
        assert_eq!(rewrite("mod.old", "mod.new($args)", &once), once);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "other(1)\n";
        assert_eq!(rewrite("old_fn", "new_fn($args)", src), src);
    }
}
