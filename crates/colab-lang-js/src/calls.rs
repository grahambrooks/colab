//! Rewrite JavaScript/TypeScript call expressions using a template.
//!
//! `match js::call "mod.old" { replace_call "..." }` finds every
//! matching call and rewrites it — see [`colab_core::render_call_template`]
//! for the placeholder language.
//!
//! Matching is exact equality on the callee's source text, so
//! `mod.old` and a bare method name are distinct targets.
//!
//! Idempotency: a template that keeps the callee text matches its own
//! output and re-applies on every pass. Rename the function to stay
//! re-runnable.

use std::fmt;
use std::path::Path;

use colab_core::Operation;

#[derive(Debug)]
pub struct CallReplace {
    pub function: String,
    pub template: String,
}

impl fmt::Display for CallReplace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "js::call \"{}\" -> replace_call \"{}\"",
            self.function, self.template
        )
    }
}

impl Operation for CallReplace {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::imports::is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        rewrite(&self.function, &self.template, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.function)
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.function)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        crate::parse(source)
            .map(|tree| {
                colab_rewrite::field_texts(
                    &tree,
                    source,
                    &["call_expression"],
                    Some("function"),
                    &[],
                )
            })
            .unwrap_or_default()
    }
}

pub fn rewrite(function: &str, template: &str, source_code: &str) -> String {
    match crate::parse(source_code) {
        Some(tree) => {
            colab_rewrite::calls::rewrite(&tree, source_code, "call_expression", function, template)
        }
        None => source_code.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_a_plain_call() {
        let src = "oldFn(a, b);\n";
        assert_eq!(rewrite("oldFn", "newFn($args)", src), "newFn(a, b);\n");
    }

    #[test]
    fn renames_a_member_call() {
        let src = "mod.old(x);\n";
        assert_eq!(rewrite("mod.old", "mod.new($args)", src), "mod.new(x);\n");
    }

    #[test]
    fn a_bare_name_does_not_match_a_member_call() {
        let src = "mod.old(x);\n";
        assert_eq!(rewrite("old", "WRONG($args)", src), src);
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "oldFn(a, b);\n";
        assert_eq!(
            rewrite("oldFn", "newFn($2, $1, null)", src),
            "newFn(b, a, null);\n"
        );
    }

    #[test]
    fn works_on_typescript_sources_too() {
        let src = "const x: number = oldFn(1);\n";
        assert_eq!(
            rewrite("oldFn", "newFn($args)", src),
            "const x: number = newFn(1);\n"
        );
    }

    #[test]
    fn handles_zero_args() {
        assert_eq!(rewrite("x", "y($args)", "x();\n"), "y();\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_the_function_is_renamed() {
        let src = "mod.old(a);\n";
        let once = rewrite("mod.old", "mod.new($args)", src);
        assert_eq!(rewrite("mod.old", "mod.new($args)", &once), once);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "other(1);\n";
        assert_eq!(rewrite("oldFn", "newFn($args)", src), src);
    }
}
