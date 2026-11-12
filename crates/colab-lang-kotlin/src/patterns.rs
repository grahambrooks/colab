//! `kotlin::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Kotlin's grammar, expando character, and the contexts that
//! let a fragment that is not a valid Kotlin *file* on its own parse.

use std::fmt;
use std::path::Path;

use colab_core::find::Located;
use colab_core::{AstInput, Error, Operation, Result, SyntaxTree};
use colab_rewrite::pattern::{PatternAction, PatternLanguage, PatternRule};

/// `$` starts a string template in Kotlin, so metavariables are parsed as
/// `µ`. Contexts, in order: a statement or expression inside a function
/// body, then a class member.
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_kotlin_ng::LANGUAGE.into(),
        expando: 'µ',
        contexts: &["fun f() {\n@@PATTERN@@\n}\n", "class C {\n@@PATTERN@@\n}\n"],
    }
}

/// `colab ast` for kotlin: a snippet's tree, or a pattern's as compiled.
pub(crate) fn syntax_tree(input: AstInput<'_>) -> Result<SyntaxTree> {
    let (text, is_pattern) = match input {
        AstInput::Source(text) => (text, false),
        AstInput::Pattern(text) => (text, true),
    };
    let report = colab_rewrite::pattern::describe(&language(), text, is_pattern)
        .map_err(|e| Error::UnsupportedOperation(format!("kotlin::pattern: {e}")))?;
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
            .map_err(|e| Error::UnsupportedOperation(format!("kotlin::pattern: {e}")))
    }
}

impl fmt::Display for PatternOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "kotlin::{}", self.0)
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
            "Log.d($TAG, $MSG)",
            PatternAction::Replace("Timber.tag($TAG).d($MSG)".into()),
        )
        .unwrap();
        let src = "fun load() {\n    Log.d(TAG, \"loading\")\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "fun load() {\n    Timber.tag(TAG).d(\"loading\")\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "kotlin::pattern 'Log.d($TAG, $MSG)' -> 'Timber.tag($TAG).d($MSG)'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = PatternOp::compile(
            "println($$$ARGS)",
            PatternAction::Replace("logger.info($$$ARGS)".into()),
        )
        .unwrap();
        let src = "fun main() {\n    println(\"hello\")\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "fun main() {\n    logger.info(\"hello\")\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = PatternOp::compile("println($$$ARGS)", PatternAction::Delete).unwrap();
        let src = "fun main() {\n    val x = 1\n    println(x)\n    use(x)\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "fun main() {\n    val x = 1\n    use(x)\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "fun f(): Int {\n    val y = foo.bar(1, 2)\n    return y\n}\n";
        for pattern in ["foo.bar($A, $B)", "return $X"] {
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
        let src = "fun f(): Int {\n    val y = legacy_name.bar(1, 2)\n    return y\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = PatternOp::compile("fun (", PatternAction::Delete).unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("kotlin::pattern"), "{err}");
    }
}
