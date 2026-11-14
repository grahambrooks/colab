//! `go::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Go's grammar, expando character, and the contexts that let an
//! expression or statement — not a valid Go *file* on its own — parse.

use std::fmt;
use std::path::Path;

use colab_core::find::Located;
use colab_core::{AstInput, Error, Operation, Result, SyntaxTree};
use colab_rewrite::pattern::{PatternAction, PatternLanguage, PatternRule, Qualifiers};

/// `$` cannot start a Go identifier, so metavariables are parsed as `µ`.
/// The contexts are tried in order: a statement or expression inside a
/// function body, then a top-level declaration.
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_go::LANGUAGE.into(),
        expando: 'µ',
        contexts: &[
            "package p\nfunc _() {\n@@PATTERN@@\n}\n",
            "package p\n@@PATTERN@@\n",
        ],
    }
}

/// `colab ast` for Go: a snippet's tree, or a pattern's as compiled.
pub(crate) fn syntax_tree(input: AstInput<'_>) -> Result<SyntaxTree> {
    let (text, is_pattern) = match input {
        AstInput::Source(text) => (text, false),
        AstInput::Pattern(text) => (text, true),
    };
    let report = colab_rewrite::pattern::describe(&language(), text, is_pattern)
        .map_err(|e| Error::UnsupportedOperation(format!("go::pattern: {e}")))?;
    Ok(SyntaxTree {
        tree: report.tree,
        has_error: report.has_error,
        context: report.context,
        prefilter: report.prefilter,
    })
}

/// A rewrite that introduces `X.` with no import binding `X` is a
/// conflict (see [`Qualifiers`]).
const QUALIFIERS: Qualifiers = Qualifiers {
    kind: "selector_expression",
    field: "operand",
    imported: crate::imports::imported_names,
    builtin: &[],
};

#[derive(Debug)]
pub struct PatternOp(pub PatternRule);

impl PatternOp {
    pub fn compile(
        pattern: &str,
        action: PatternAction,
        constraints: &[colab_core::Constraint],
    ) -> Result<Self> {
        PatternRule::compile(&language(), pattern, action, constraints)
            .map(|rule| Self(rule.with_qualifiers(QUALIFIERS)))
            .map_err(|e| Error::UnsupportedOperation(format!("go::pattern: {e}")))
    }
}

impl fmt::Display for PatternOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "go::{}", self.0)
    }
}

impl Operation for PatternOp {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("go")
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
    fn rewrites_errors_wrap_to_fmt_errorf() {
        let op = PatternOp::compile(
            "errors.Wrap($ERR, $MSG)",
            PatternAction::Replace("fmt.Errorf(\"%s: %w\", $MSG, $ERR)".into()),
            &[],
        )
        .unwrap();
        let src = "package main\n\nfunc f() error {\n\treturn errors.Wrap(err, \"read\")\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "package main\n\nfunc f() error {\n\treturn fmt.Errorf(\"%s: %w\", \"read\", err)\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "go::pattern 'errors.Wrap($ERR, $MSG)' -> 'fmt.Errorf(\"%s: %w\", $MSG, $ERR)'"
        );
    }

    #[test]
    fn a_qualifier_nothing_imports_is_a_conflict() {
        let op = PatternOp::compile(
            "errors.Wrap($E, $M)",
            PatternAction::Replace("fmt.Errorf($M+\": %w\", $E)".into()),
            &[],
        )
        .unwrap();
        let before = "package p\n\nimport \"github.com/pkg/errors\"\n\nfunc f() error {\n\treturn errors.Wrap(err, \"x\")\n}\n";
        let after = op.apply(before);
        let conflicts = op.conflicts(before, &after);
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert!(conflicts[0].contains("`fmt.`"));

        // Imported (here by an alias-free `fmt`), or already used: no conflict.
        let imported = after.replace(
            "import \"github.com/pkg/errors\"",
            "import (\n\t\"fmt\"\n\t\"github.com/pkg/errors\"\n)",
        );
        assert!(op.conflicts(before, &imported).is_empty());
        let used = before.replace("return", "fmt.Println()\n\treturn");
        assert!(op.conflicts(&used, &op.apply(&used)).is_empty());
        // `err.Error()` qualifies by a capture already in the file.
        let wrap = PatternOp::compile(
            "errors.Wrap($E, $M)",
            PatternAction::Replace("errors.New($E.Error())".into()),
            &[],
        )
        .unwrap();
        assert!(wrap.conflicts(before, &wrap.apply(before)).is_empty());
    }

    #[test]
    fn a_template_that_does_not_parse_explains_the_dead_rule() {
        let op =
            PatternOp::compile("f($$$A)", PatternAction::Replace("g($$$A,,)".into()), &[]).unwrap();
        let src = "package p\n\nfunc main() {\n\tf(1, 2)\n}\n";
        assert_eq!(op.apply(src), src);
        assert!(op.why_unchanged(src).unwrap().contains("would not parse"));
        assert_eq!(op.why_unchanged("package p\n"), None);
    }

    #[test]
    fn top_level_declaration_pattern_compiles() {
        let op = PatternOp::compile("var $NAME = $VALUE", PatternAction::Delete, &[]).unwrap();
        let out = op.apply("package p\n\nvar debug = true\n\nfunc main() {}\n");
        assert_eq!(out, "package p\n\n\nfunc main() {}\n");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = PatternOp::compile("func (", PatternAction::Delete, &[]).unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("go::pattern"), "{err}");
    }
}
