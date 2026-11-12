//! `csharp::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies C#'s grammar, expando character, and the contexts that
//! let a fragment that is not a valid C# *file* on its own parse.

use std::fmt;
use std::path::Path;

use colab_core::find::Located;
use colab_core::{AstInput, Error, Operation, Result, SyntaxTree};
use colab_rewrite::pattern::{PatternAction, PatternLanguage, PatternRule};

/// `$` starts an interpolated string in C#, so metavariables are parsed
/// as `µ`.
///
/// ast-grep selects the *first* node of the pattern's kind in a context, so
/// an expression is tried first in a top-level `return`, which puts no
/// identifier before the slot. Then a statement inside a method body (as
/// written, then with `;` appended), then a class member.
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_c_sharp::LANGUAGE.into(),
        expando: 'µ',
        contexts: &[
            "return @@PATTERN@@;",
            "class C {\nvoid M() {\n@@PATTERN@@\n}\n}\n",
            "class C {\nvoid M() {\n@@PATTERN@@;\n}\n}\n",
            "class C {\n@@PATTERN@@\n}\n",
        ],
    }
}

/// `colab ast` for csharp: a snippet's tree, or a pattern's as compiled.
pub(crate) fn syntax_tree(input: AstInput<'_>) -> Result<SyntaxTree> {
    let (text, is_pattern) = match input {
        AstInput::Source(text) => (text, false),
        AstInput::Pattern(text) => (text, true),
    };
    let report = colab_rewrite::pattern::describe(&language(), text, is_pattern)
        .map_err(|e| Error::UnsupportedOperation(format!("csharp::pattern: {e}")))?;
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
            .map_err(|e| Error::UnsupportedOperation(format!("csharp::pattern: {e}")))
    }
}

impl fmt::Display for PatternOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "csharp::{}", self.0)
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
            "Assert.AreEqual($A, $B)",
            PatternAction::Replace("Assert.That($B, Is.EqualTo($A))".into()),
        )
        .unwrap();
        let src =
            "class T {\n    void Test() {\n        Assert.AreEqual(4, Add(2, 2));\n    }\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "class T {\n    void Test() {\n        Assert.That(Add(2, 2), Is.EqualTo(4));\n    }\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "csharp::pattern 'Assert.AreEqual($A, $B)' -> 'Assert.That($B, Is.EqualTo($A))'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = PatternOp::compile(
            "Console.WriteLine($$$ARGS)",
            PatternAction::Replace("logger.LogInformation($$$ARGS)".into()),
        )
        .unwrap();
        let src = "class P {\n    void Run(int n) {\n        Console.WriteLine(\"n={0}\", n);\n    }\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "class P {\n    void Run(int n) {\n        logger.LogInformation(\"n={0}\", n);\n    }\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = PatternOp::compile("Debug.WriteLine($X)", PatternAction::Delete).unwrap();
        let src = "class P {\n    void Run(int n) {\n        Debug.WriteLine(n);\n        Use(n);\n    }\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "class P {\n    void Run(int n) {\n        Use(n);\n    }\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src =
            "class C {\n    void M() {\n        var y = foo.Bar(1, 2);\n        y = 3;\n    }\n}\n";
        for pattern in ["foo.Bar($A, $B)", "$X = $Y;"] {
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
        let src = "class C {\n    void M() {\n        var y = legacy_name.Bar(1, 2);\n        y = 3;\n    }\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = PatternOp::compile("class (", PatternAction::Delete).unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("csharp::pattern"), "{err}");
    }
}
