//! `php::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies PHP's grammar, expando character, and the contexts that
//! let a fragment that is not a valid PHP *file* on its own parse.

use std::fmt;
use std::path::Path;

use colab_core::find::Located;
use colab_core::{AstInput, Error, Operation, Result, SyntaxTree};
use colab_rewrite::pattern::{PatternAction, PatternLanguage, PatternRule};

/// `$x` is a PHP variable, so metavariables are `$UPPER` and parse as `µ`
/// (a PHP name); a lower-case `$x` stays a variable.
///
/// This uses the PHP-only grammar, not the HTML-embedding one the other
/// `php::` operations use: under that grammar any text outside `<?php` is
/// inline HTML, so a bare pattern would "parse" as one opaque text node. The
/// PHP-only grammar still accepts a leading `<?php` tag, so ordinary files
/// match normally; only inline HTML around the code is seen as an error
/// region (matches elsewhere in the file are unaffected). Contexts, in
/// order: an expression made a statement with `;`, then a class member.
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_php::LANGUAGE_PHP_ONLY.into(),
        expando: 'µ',
        contexts: &["@@PATTERN@@;", "class C {\n@@PATTERN@@\n}\n"],
    }
}

/// `colab ast` for php: a snippet's tree, or a pattern's as compiled.
pub(crate) fn syntax_tree(input: AstInput<'_>) -> Result<SyntaxTree> {
    let (text, is_pattern) = match input {
        AstInput::Source(text) => (text, false),
        AstInput::Pattern(text) => (text, true),
    };
    let report = colab_rewrite::pattern::describe(&language(), text, is_pattern)
        .map_err(|e| Error::UnsupportedOperation(format!("php::pattern: {e}")))?;
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
            .map_err(|e| Error::UnsupportedOperation(format!("php::pattern: {e}")))
    }
}

impl fmt::Display for PatternOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "php::{}", self.0)
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
            "$this->assertEquals($A, $B)",
            PatternAction::Replace("$this->assertSame($A, $B)".into()),
        )
        .unwrap();
        let src = "<?php\nclass T extends TestCase {\n    public function testAdd() {\n        $this->assertEquals(4, add(2, 2));\n    }\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "<?php\nclass T extends TestCase {\n    public function testAdd() {\n        $this->assertSame(4, add(2, 2));\n    }\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "php::pattern '$this->assertEquals($A, $B)' -> '$this->assertSame($A, $B)'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = PatternOp::compile(
            "var_dump($$$ARGS)",
            PatternAction::Replace("dump($$$ARGS)".into()),
        )
        .unwrap();
        let src = "<?php\nvar_dump($user, $order);\n";
        let out = op.apply(src);
        assert_eq!(out, "<?php\ndump($user, $order);\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = PatternOp::compile("var_dump($X)", PatternAction::Delete).unwrap();
        let src = "<?php\nfunction f($x) {\n    var_dump($x);\n    return $x;\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "<?php\nfunction f($x) {\n    return $x;\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "<?php\nfunction f() {\n    $z = $foo->bar(1, 2);\n    $y = 3;\n}\n";
        for pattern in ["$foo->bar($A, $B)", "$y = $X;"] {
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
        let src = "<?php\nfunction f() {\n    $z = $legacy_name->bar(1, 2);\n    $y = 3;\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = PatternOp::compile("function (", PatternAction::Delete).unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("php::pattern"), "{err}");
    }

    #[test]
    fn matches_code_after_inline_html() {
        let op = PatternOp::compile("var_dump($$$ARGS)", PatternAction::Delete).unwrap();
        let src =
            "<ul>\n<?php\nvar_dump($items);\nforeach ($items as $i) { echo $i; }\n?>\n</ul>\n";
        assert_eq!(
            op.apply(src),
            "<ul>\n<?php\nforeach ($items as $i) { echo $i; }\n?>\n</ul>\n"
        );
    }
}
