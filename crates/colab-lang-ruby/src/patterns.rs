//! `ruby::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Ruby's grammar, expando character, and the contexts that
//! let a fragment that is not a valid Ruby *file* on its own parse.

use std::fmt;
use std::path::Path;

use colab_core::find::Located;
use colab_core::{AstInput, Error, Operation, Result, SyntaxTree};
use colab_rewrite::pattern::{PatternAction, PatternLanguage, PatternRule};

/// `$x` is a Ruby global variable, so metavariables are parsed as `µ`.
/// Expressions and statements parse on their own; the contexts are a method
/// body and a class body, for fragments that need one.
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_ruby::LANGUAGE.into(),
        expando: 'µ',
        contexts: &["def _\n@@PATTERN@@\nend\n", "class C\n@@PATTERN@@\nend\n"],
    }
}

/// `colab ast` for ruby: a snippet's tree, or a pattern's as compiled.
pub(crate) fn syntax_tree(input: AstInput<'_>) -> Result<SyntaxTree> {
    let (text, is_pattern) = match input {
        AstInput::Source(text) => (text, false),
        AstInput::Pattern(text) => (text, true),
    };
    let report = colab_rewrite::pattern::describe(&language(), text, is_pattern)
        .map_err(|e| Error::UnsupportedOperation(format!("ruby::pattern: {e}")))?;
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
            .map_err(|e| Error::UnsupportedOperation(format!("ruby::pattern: {e}")))
    }
}

impl fmt::Display for PatternOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ruby::{}", self.0)
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
            "$X.should == $Y",
            PatternAction::Replace("expect($X).to eq($Y)".into()),
        )
        .unwrap();
        let src = "it \"adds\" do\n  add(2, 2).should == 4\nend\n";
        let out = op.apply(src);
        assert_eq!(out, "it \"adds\" do\n  expect(add(2, 2)).to eq(4)\nend\n");
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "ruby::pattern '$X.should == $Y' -> 'expect($X).to eq($Y)'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = PatternOp::compile(
            "puts($$$ARGS)",
            PatternAction::Replace("Rails.logger.info($$$ARGS)".into()),
        )
        .unwrap();
        let src = "def run(n)\n  puts(\"n=\", n)\nend\n";
        let out = op.apply(src);
        assert_eq!(out, "def run(n)\n  Rails.logger.info(\"n=\", n)\nend\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = PatternOp::compile("binding.pry", PatternAction::Delete).unwrap();
        let src = "def run(n)\n  binding.pry\n  n * 2\nend\n";
        let out = op.apply(src);
        assert_eq!(out, "def run(n)\n  n * 2\nend\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "def f\n  y = foo.bar(1, 2)\n  return y\nend\n";
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
        let src = "def f\n  y = legacy_name.bar(1, 2)\n  return y\nend\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = PatternOp::compile("def (", PatternAction::Delete).unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("ruby::pattern"), "{err}");
    }
}
