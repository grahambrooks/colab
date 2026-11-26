//! Rewrite Java call expressions using a template.
//!
//! `method_invocation` carries an optional `object` field, so the callee
//! is read from the source span between the start of the call and its
//! argument list. Targets read exactly as the call is written:
//!
//! | Source | Target |
//! | ------ | ------ |
//! | `helper(x)` | `"helper"` |
//! | `Old.run(a)` | `"Old.run"` |
//! | `this.foo(y)` | `"this.foo"` |
//!
//! Qualified and bare forms are therefore distinct targets and never
//! match each other.
//!
//! Idempotency: templates that keep the callee text match their own
//! output and re-apply on every pass. Rename the method to stay
//! re-runnable.
//!
//! The floor reads the callee as the source before the argument list,
//! trailing whitespace trimmed (`CallShape::Prefixed`): taking it as raw
//! source keeps the receiver in the target, which is what makes qualified
//! and bare calls distinguishable.

#[cfg(test)]
fn rewrite(function: &str, template: &str, source_code: &str) -> String {
    crate::LANG.rewrite_calls(function, template, source_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_a_qualified_call() {
        let src = "class A { void m() { Old.run(a, b); } }\n";
        let out = rewrite("Old.run", "New.run($args)", src);
        assert_eq!(out, "class A { void m() { New.run(a, b); } }\n");
    }

    #[test]
    fn renames_a_bare_call() {
        let src = "class A { void m() { helper(x); } }\n";
        let out = rewrite("helper", "newHelper($args)", src);
        assert_eq!(out, "class A { void m() { newHelper(x); } }\n");
    }

    #[test]
    fn a_bare_name_does_not_match_a_qualified_call() {
        let src = "class A { void m() { Old.run(a); } }\n";
        assert_eq!(rewrite("run", "WRONG($args)", src), src);
    }

    #[test]
    fn matches_a_this_qualified_call() {
        let src = "class A { void m() { this.foo(y); } }\n";
        let out = rewrite("this.foo", "this.bar($args)", src);
        assert_eq!(out, "class A { void m() { this.bar(y); } }\n");
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "class A { void m() { Old.run(a, b); } }\n";
        let out = rewrite("Old.run", "New.run($2, $1, null)", src);
        assert_eq!(out, "class A { void m() { New.run(b, a, null); } }\n");
    }

    #[test]
    fn handles_zero_args() {
        let src = "class A { void m() { x(); } }\n";
        assert_eq!(
            rewrite("x", "y($args)", src),
            "class A { void m() { y(); } }\n"
        );
    }

    #[test]
    fn rewrite_is_idempotent_when_the_method_is_renamed() {
        let src = "class A { void m() { Old.run(a); } }\n";
        let once = rewrite("Old.run", "New.run($args)", src);
        assert_eq!(rewrite("Old.run", "New.run($args)", &once), once);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "class A { void m() { other(1); } }\n";
        assert_eq!(rewrite("Old.run", "New.run($args)", src), src);
    }
}
