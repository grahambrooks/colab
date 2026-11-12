//! `cpp::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies C++'s grammar, expando character, and the contexts that
//! let a fragment that is not a valid C++ *file* on its own parse.

use std::fmt;
use std::path::Path;

use colab_core::find::Located;
use colab_core::{AstInput, Error, Operation, Result, SyntaxTree};
use colab_rewrite::pattern::{PatternAction, PatternLanguage, PatternRule};

/// `$` can appear in C++ identifiers (a compiler extension), so
/// metavariables are parsed as `µ`.
///
/// ast-grep selects the *first* node of the pattern's kind in a context, so
/// the first context, a top-level expression statement, has nothing before
/// the slot; the function-body contexts, for statements that need one,
/// follow (as written, then with `;` appended).
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_cpp::LANGUAGE.into(),
        expando: 'µ',
        contexts: &[
            "@@PATTERN@@;",
            "void _() {\n@@PATTERN@@\n}\n",
            "void _() {\n@@PATTERN@@;\n}\n",
        ],
    }
}

/// `colab ast` for cpp: a snippet's tree, or a pattern's as compiled.
pub(crate) fn syntax_tree(input: AstInput<'_>) -> Result<SyntaxTree> {
    let (text, is_pattern) = match input {
        AstInput::Source(text) => (text, false),
        AstInput::Pattern(text) => (text, true),
    };
    let report = colab_rewrite::pattern::describe(&language(), text, is_pattern)
        .map_err(|e| Error::UnsupportedOperation(format!("cpp::pattern: {e}")))?;
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
            .map_err(|e| Error::UnsupportedOperation(format!("cpp::pattern: {e}")))
    }
}

impl fmt::Display for PatternOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cpp::{}", self.0)
    }
}

impl Operation for PatternOp {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
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
            "std::make_pair($A, $B)",
            PatternAction::Replace("std::pair($A, $B)".into()),
        )
        .unwrap();
        let src = "auto f(int k, int v) {\n    return std::make_pair(k, v);\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "auto f(int k, int v) {\n    return std::pair(k, v);\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "cpp::pattern 'std::make_pair($A, $B)' -> 'std::pair($A, $B)'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = PatternOp::compile(
            "std::printf($$$ARGS)",
            PatternAction::Replace("fmt::print($$$ARGS)".into()),
        )
        .unwrap();
        let src = "void f(int n) {\n    std::printf(\"%d\\n\", n);\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "void f(int n) {\n    fmt::print(\"%d\\n\", n);\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = PatternOp::compile("DEBUG_TRACE($X)", PatternAction::Delete).unwrap();
        let src = "void f(int x) {\n    DEBUG_TRACE(x);\n    use(x);\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "void f(int x) {\n    use(x);\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "void f() {\n    int y = foo.bar(1, 2);\n    y = 3;\n}\n";
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
        let src = "void f() {\n    int y = legacy_name.bar(1, 2);\n    y = 3;\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = PatternOp::compile("void (", PatternAction::Delete).unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("cpp::pattern"), "{err}");
    }
}
