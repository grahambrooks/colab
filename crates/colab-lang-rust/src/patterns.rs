//! `rust::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Rust's grammar, expando character, and the contexts that
//! let a fragment that is not a valid Rust *file* on its own parse.

use std::fmt;
use std::path::Path;

use colab_core::find::Located;
use colab_core::{AstInput, Error, Operation, Result, SyntaxTree};
use colab_rewrite::pattern::{PatternAction, PatternLanguage, PatternRule};

/// `$` only appears in Rust inside `macro_rules!`, where it means a macro
/// variable, so metavariables are parsed as `µ`. Items and `;`-terminated
/// statements parse on their own.
///
/// ast-grep selects the *first* node of the pattern's kind in a context, so
/// an expression is tried first as a top-level statement (nothing before
/// the slot), then as the tail of a function body.
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_rust::LANGUAGE.into(),
        expando: 'µ',
        contexts: &["@@PATTERN@@;", "fn f() {\n@@PATTERN@@\n}\n"],
    }
}

/// `colab ast` for rust: a snippet's tree, or a pattern's as compiled.
pub(crate) fn syntax_tree(input: AstInput<'_>) -> Result<SyntaxTree> {
    let (text, is_pattern) = match input {
        AstInput::Source(text) => (text, false),
        AstInput::Pattern(text) => (text, true),
    };
    let report = colab_rewrite::pattern::describe(&language(), text, is_pattern)
        .map_err(|e| Error::UnsupportedOperation(format!("rust::pattern: {e}")))?;
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
            .map_err(|e| Error::UnsupportedOperation(format!("rust::pattern: {e}")))
    }
}

impl fmt::Display for PatternOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rust::{}", self.0)
    }
}

impl Operation for PatternOp {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("rs")
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
            "$MAP.get($KEY).is_some()",
            PatternAction::Replace("$MAP.contains_key($KEY)".into()),
            &[],
        )
        .unwrap();
        let src = "fn f(m: &HashMap<String, u32>, k: &str) -> bool {\n    m.get(k).is_some()\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "fn f(m: &HashMap<String, u32>, k: &str) -> bool {\n    m.contains_key(k)\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "rust::pattern '$MAP.get($KEY).is_some()' -> '$MAP.contains_key($KEY)'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = PatternOp::compile(
            "Rc::new($$$ARGS)",
            PatternAction::Replace("Arc::new($$$ARGS)".into()),
            &[],
        )
        .unwrap();
        let src = "fn f() {\n    let a = Rc::new(Node { id: 1 });\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "fn f() {\n    let a = Arc::new(Node { id: 1 });\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = PatternOp::compile("dbg!($X)", PatternAction::Delete, &[]).unwrap();
        let src = "fn f(x: u32) -> u32 {\n    dbg!(x);\n    x + 1\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "fn f(x: u32) -> u32 {\n    x + 1\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "fn f() {\n    let y = foo.bar(1, 2);\n    y\n}\n";
        for pattern in ["foo.bar($A, $B)", "let $X = $Y;"] {
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
        let src = "fn f() {\n    let y = legacy_name.bar(1, 2);\n    y\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = PatternOp::compile("fn (", PatternAction::Delete, &[]).unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("rust::pattern"), "{err}");
    }
}
