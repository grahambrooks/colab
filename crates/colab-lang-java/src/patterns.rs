//! `java::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Java's grammar, expando character, and the contexts that
//! let a fragment that is not a valid Java *file* on its own parse.

use colab_rewrite::pattern::PatternLanguage;

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
        also: &[],
        qualified_calls: None,
        // Javadoc documents the declaration after it; annotations are
        // already part of the declaration's modifiers.
        leading: colab_rewrite::pattern::Leading {
            kinds: &[],
            doc_comments: &["/**"],
        },
        macro_contents: None,
        visibilities: &[],
    }
}

#[cfg(test)]
mod tests {
    use colab_core::{Error, Operation};
    use colab_rewrite::pattern::PatternAction;

    #[test]
    fn a_deleted_method_takes_its_javadoc() {
        let op = crate::LANG
            .compile_pattern("void $F() {}", PatternAction::Delete, &[])
            .unwrap();
        let src = "class A {\n    // a note\n    /** Old. */\n    void old() {}\n    int kept() { return 1; }\n}\n";
        assert_eq!(
            op.apply(src),
            "class A {\n    // a note\n    int kept() { return 1; }\n}\n"
        );
    }

    #[test]
    fn rewrites_call_with_two_captures() {
        let op = crate::LANG
            .compile_pattern(
                "Assert.assertEquals($A, $B)",
                PatternAction::Replace("assertThat($B).isEqualTo($A)".into()),
                &[],
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
        let op = crate::LANG
            .compile_pattern(
                "System.out.printf($$$ARGS)",
                PatternAction::Replace("logger.info($$$ARGS)".into()),
                &[],
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
        let op = crate::LANG
            .compile_pattern("System.out.println($X)", PatternAction::Delete, &[])
            .unwrap();
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
            let op = crate::LANG
                .compile_pattern(pattern, PatternAction::Delete, &[])
                .unwrap();
            let found = op.locate(src).unwrap();
            assert_eq!(found.len(), 1, "{pattern}");
        }
    }

    #[test]
    fn a_bare_identifier_compiles() {
        // The selector must be the pattern's own identifier, not the first
        // identifier of whichever context it was parsed in.
        let op = crate::LANG
            .compile_pattern("legacy_name", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(op.prefilter(), Some("legacy_name"));
        let src = "class C {\n    void m() {\n        int y = legacy_name.bar(1, 2);\n        y = 3;\n    }\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = crate::LANG
            .compile_pattern("class (", PatternAction::Delete, &[])
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("java::pattern"), "{err}");
    }
}
