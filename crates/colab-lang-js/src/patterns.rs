//! `js::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies JavaScript's grammar, expando character, and the contexts that
//! let a fragment that is not a valid JavaScript *file* on its own parse.

use std::fmt;
use std::path::Path;

use colab_core::find::Located;
use colab_core::{AstInput, Error, Operation, Result, SyntaxTree};
use colab_rewrite::pattern::{PatternAction, PatternLanguage, PatternRule};

/// `$` is a legal JavaScript identifier character, so metavariables parse
/// as identifiers unchanged. Expressions and statements parse on their own;
/// the one context admits a class member such as a method.
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_javascript::LANGUAGE.into(),
        expando: '$',
        contexts: &["class C {\n@@PATTERN@@\n}\n"],
    }
}

/// `colab ast` for js: a snippet's tree, or a pattern's as compiled.
pub(crate) fn syntax_tree(input: AstInput<'_>) -> Result<SyntaxTree> {
    let (text, is_pattern) = match input {
        AstInput::Source(text) => (text, false),
        AstInput::Pattern(text) => (text, true),
    };
    let report = colab_rewrite::pattern::describe(&language(), text, is_pattern)
        .map_err(|e| Error::UnsupportedOperation(format!("js::pattern: {e}")))?;
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
    pub fn compile(
        pattern: &str,
        action: PatternAction,
        constraints: &[colab_core::Constraint],
    ) -> Result<Self> {
        PatternRule::compile(&language(), pattern, action, constraints)
            .map(Self)
            .map_err(|e| Error::UnsupportedOperation(format!("js::pattern: {e}")))
    }
}

impl fmt::Display for PatternOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "js::{}", self.0)
    }
}

impl Operation for PatternOp {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::imports::is_relevant(path)
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

    fn conflicts(&self, before: &str, after: &str) -> Vec<String> {
        self.0.conflicts(before, after)
    }

    fn why_unchanged(&self, source: &str) -> Option<String> {
        self.0.why_unchanged(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_call_with_two_captures() {
        let op = PatternOp::compile(
            "$A.indexOf($B) !== -1",
            PatternAction::Replace("$A.includes($B)".into()),
            &[],
        )
        .unwrap();
        let src = "if (names.indexOf(name) !== -1) {\n  greet(name);\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "if (names.includes(name)) {\n  greet(name);\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "js::pattern '$A.indexOf($B) !== -1' -> '$A.includes($B)'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = PatternOp::compile(
            "console.log($$$ARGS)",
            PatternAction::Replace("logger.info($$$ARGS)".into()),
            &[],
        )
        .unwrap();
        let src = "function f(a, b) {\n  console.log('sum', a + b);\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "function f(a, b) {\n  logger.info('sum', a + b);\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = PatternOp::compile("console.debug($$$ARGS)", PatternAction::Delete, &[]).unwrap();
        let src = "function f(x) {\n  console.debug('x =', x);\n  return x;\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "function f(x) {\n  return x;\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "function f() {\n  let y = foo.bar(1, 2);\n  y = 3;\n}\n";
        for pattern in ["foo.bar($A, $B)", "$X = $Y;"] {
            let op = PatternOp::compile(pattern, PatternAction::Delete, &[]).unwrap();
            let found = op.locate(src).unwrap();
            assert_eq!(found.len(), 1, "{pattern}");
        }
    }

    #[test]
    fn a_bare_identifier_compiles() {
        // The selector must be the pattern's own identifier, not the first
        // identifier of whichever context it was parsed in.
        let op = PatternOp::compile("legacy_name", PatternAction::Delete, &[]).unwrap();
        assert_eq!(op.prefilter(), Some("legacy_name"));
        let src = "function f() {\n  let y = legacy_name.bar(1, 2);\n  y = 3;\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = PatternOp::compile("function (", PatternAction::Delete, &[]).unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("js::pattern"), "{err}");
    }
}
