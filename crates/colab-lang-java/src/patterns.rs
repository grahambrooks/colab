//! `java::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Java's grammar, expando character, and the contexts that
//! let a fragment that is not a valid Java *file* on its own parse.

use std::fmt;
use std::path::Path;

use colab_core::find::Located;
use colab_core::{AstInput, Error, Operation, Result, SyntaxTree};
use colab_rewrite::pattern::{PatternAction, PatternLanguage, PatternRule};

/// `$` is a legal Java identifier character with no other meaning, so
/// metavariables parse as identifiers unchanged.
///
/// ast-grep selects the *first* node of the pattern's kind in a context, so
/// an expression is tried first as a top-level statement, with nothing
/// before the slot. Then a statement inside a method body (as written, then
/// with `;` appended), then a class member.
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_java::LANGUAGE.into(),
        expando: '$',
        contexts: &[
            "@@PATTERN@@;",
            "class C {\nvoid m() {\n@@PATTERN@@\n}\n}\n",
            "class C {\nvoid m() {\n@@PATTERN@@;\n}\n}\n",
            "class C {\n@@PATTERN@@\n}\n",
        ],
    }
}

/// `colab ast` for java: a snippet's tree, or a pattern's as compiled.
pub(crate) fn syntax_tree(input: AstInput<'_>) -> Result<SyntaxTree> {
    let (text, is_pattern) = match input {
        AstInput::Source(text) => (text, false),
        AstInput::Pattern(text) => (text, true),
    };
    let report = colab_rewrite::pattern::describe(&language(), text, is_pattern)
        .map_err(|e| Error::UnsupportedOperation(format!("java::pattern: {e}")))?;
    Ok(SyntaxTree {
        tree: report.tree,
        has_error: report.has_error,
        context: report.context,
        prefilter: report.prefilter,
    })
}

#[derive(Debug)]
pub struct PatternOp(pub PatternRule);

impl PatternOp {
    pub fn compile(pattern: &str, action: PatternAction) -> Result<Self> {
        PatternRule::compile(&language(), pattern, action)
            .map(Self)
            .map_err(|e| Error::UnsupportedOperation(format!("java::pattern: {e}")))
    }
}

impl fmt::Display for PatternOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "java::{}", self.0)
    }
}

impl Operation for PatternOp {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("java")
    }

    fn apply(&self, source_code: &str) -> String {
        self.0.apply(source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        self.0.prefilter()
    }

    fn locate(&self, source: &str) -> Option<Vec<Located>> {
        Some(
            self.0
                .find(source)
                .into_iter()
                .map(|f| Located::at(source, f.start, f.end, f.captures))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_call_with_two_captures() {
        let op = PatternOp::compile(
            "Assert.assertEquals($A, $B)",
            PatternAction::Replace("assertThat($B).isEqualTo($A)".into()),
        )
        .unwrap();
        let src =
            "class T {\n    void test() {\n        Assert.assertEquals(4, add(2, 2));\n    }\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "class T {\n    void test() {\n        assertThat(add(2, 2)).isEqualTo(4);\n    }\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "java::pattern 'Assert.assertEquals($A, $B)' -> 'assertThat($B).isEqualTo($A)'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = PatternOp::compile(
            "System.out.printf($$$ARGS)",
            PatternAction::Replace("logger.info($$$ARGS)".into()),
        )
        .unwrap();
        let src = "class P {\n    void run(int n) {\n        System.out.printf(\"n=%d%n\", n);\n    }\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "class P {\n    void run(int n) {\n        logger.info(\"n=%d%n\", n);\n    }\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = PatternOp::compile("System.out.println($X)", PatternAction::Delete).unwrap();
        let src = "class P {\n    void run(int n) {\n        System.out.println(n);\n        use(n);\n    }\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "class P {\n    void run(int n) {\n        use(n);\n    }\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src =
            "class C {\n    void m() {\n        int y = foo.bar(1, 2);\n        y = 3;\n    }\n}\n";
        for pattern in ["foo.bar($A, $B)", "$X = $Y;"] {
            let op = PatternOp::compile(pattern, PatternAction::Delete).unwrap();
            let found = op.locate(src).unwrap();
            assert_eq!(found.len(), 1, "{pattern}");
        }
    }

    #[test]
    fn a_bare_identifier_compiles() {
        // The selector must be the pattern's own identifier, not the first
        // identifier of whichever context it was parsed in.
        let op = PatternOp::compile("legacy_name", PatternAction::Delete).unwrap();
        assert_eq!(op.prefilter(), Some("legacy_name"));
        let src = "class C {\n    void m() {\n        int y = legacy_name.bar(1, 2);\n        y = 3;\n    }\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = PatternOp::compile("class (", PatternAction::Delete).unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("java::pattern"), "{err}");
    }
}
