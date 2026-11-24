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

fn rewrite(function: &str, template: &str, source_code: &str) -> String {
    crate::LANG.rewrite_calls(function, template, source_code)
}

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
        assert_eq!(
            rewrite("X", "Y($args)", src),
            "class C { void M() { Y(); } }\n"
        );
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
