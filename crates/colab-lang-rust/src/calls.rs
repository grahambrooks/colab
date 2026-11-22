//! Rewrite Rust call expressions using a template.
//!
//! Mirrors `colab-lang-go::calls`. Tree-sitter-rust represents
//! function calls as `call_expression` with `function` and
//! `arguments` fields; we match on the `function` node's source text
//! exactly. Method calls like `x.foo()` are *method_call_expression*
//! nodes — those are excluded so a rule targeting `foo` does not
//! collide with a method named `foo`.

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
            "rust::call \"{}\" -> replace_call \"{}\"",
            self.function, self.template
        )
    }
}

impl Operation for CallReplace {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("rs")
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
        crate::parse(source).map_or_default(|tree| {
            colab_rewrite::field_texts(&tree, source, &["call_expression"], Some("function"), &[])
        })
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
    fn renames_a_function_call_with_args_passthrough() {
        let src = "fn main() { pkg::old(a, b); }\n";
        let out = rewrite("pkg::old", "pkg::new($args)", src);
        assert_eq!(out, "fn main() { pkg::new(a, b); }\n");
    }

    #[test]
    fn reorders_args_via_positional_placeholders() {
        let src = "fn main() { pkg::old(a, b); }\n";
        let out = rewrite("pkg::old", "pkg::new($2, $1, None)", src);
        assert_eq!(out, "fn main() { pkg::new(b, a, None); }\n");
    }

    #[test]
    fn ignores_method_calls() {
        // `x.foo()` is a method_call_expression, not a call_expression
        // whose function field equals "foo".
        let src = "fn main() { x.foo(1); }\n";
        assert_eq!(rewrite("foo", "bar($args)", src), src);
    }

    #[test]
    fn handles_zero_args() {
        let src = "fn main() { trace(); }\n";
        let out = rewrite("trace", "log($args)", src);
        assert_eq!(out, "fn main() { log(); }\n");
    }

    #[test]
    fn rewrite_is_idempotent_when_function_renamed() {
        let src = "fn main() { pkg::old(a); }\n";
        let once = rewrite("pkg::old", "pkg::new($args)", src);
        let twice = rewrite("pkg::old", "pkg::new($args)", &once);
        assert_eq!(once, twice);
    }
}
