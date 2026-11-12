//! `c::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies C's grammar, expando character, and the contexts that
//! let a fragment that is not a valid C *file* on its own parse.

use std::fmt;
use std::path::Path;

use colab_core::find::Located;
use colab_core::{AstInput, Error, Operation, Result, SyntaxTree};
use colab_rewrite::pattern::{PatternAction, PatternLanguage, PatternRule};

/// `$` can appear in C identifiers (a GCC extension), so metavariables are
/// parsed as `µ`.
///
/// ast-grep selects the *first* node of the pattern's kind in a context, so
/// a context must not put a node the pattern could be (an `identifier`)
/// before the slot. The first context, a top-level expression statement,
/// has nothing before it; the function-body contexts, for statements that
/// need one, follow (as written, then with `;` appended).
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_c::LANGUAGE.into(),
        expando: 'µ',
        contexts: &[
            "@@PATTERN@@;",
            "void _(void) {\n@@PATTERN@@\n}\n",
            "void _(void) {\n@@PATTERN@@;\n}\n",
        ],
    }
}

/// `colab ast` for c: a snippet's tree, or a pattern's as compiled.
pub(crate) fn syntax_tree(input: AstInput<'_>) -> Result<SyntaxTree> {
    let (text, is_pattern) = match input {
        AstInput::Source(text) => (text, false),
        AstInput::Pattern(text) => (text, true),
    };
    let report = colab_rewrite::pattern::describe(&language(), text, is_pattern)
        .map_err(|e| Error::UnsupportedOperation(format!("c::pattern: {e}")))?;
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
            .map_err(|e| Error::UnsupportedOperation(format!("c::pattern: {e}")))
    }
}

impl fmt::Display for PatternOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "c::{}", self.0)
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
            "strcpy($DST, $SRC)",
            PatternAction::Replace("strlcpy($DST, $SRC, sizeof($DST))".into()),
        )
        .unwrap();
        let src = "void f(char *d, const char *s) {\n    strcpy(d, s);\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "void f(char *d, const char *s) {\n    strlcpy(d, s, sizeof(d));\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "c::pattern 'strcpy($DST, $SRC)' -> 'strlcpy($DST, $SRC, sizeof($DST))'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = PatternOp::compile(
            "printf($$$ARGS)",
            PatternAction::Replace("log_info($$$ARGS)".into()),
        )
        .unwrap();
        let src = "int main(void) {\n    printf(\"%d %s\\n\", n, name);\n    return 0;\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "int main(void) {\n    log_info(\"%d %s\\n\", n, name);\n    return 0;\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = PatternOp::compile("debug_dump($X)", PatternAction::Delete).unwrap();
        let src = "void f(void) {\n    int x = 1;\n    debug_dump(x);\n    use(x);\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "void f(void) {\n    int x = 1;\n    use(x);\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "void f(void) {\n    int y = foo.bar(1, 2);\n    y = 3;\n}\n";
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
        let src = "void f(void) {\n    int y = legacy_name.bar(1, 2);\n    y = 3;\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = PatternOp::compile("void (", PatternAction::Delete).unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("c::pattern"), "{err}");
    }
}
