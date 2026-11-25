//! `java::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Java's grammar, expando character, and the contexts that
//! let a fragment that is not a valid Java *file* on its own parse.

use colab_rewrite::pattern::PatternLanguage;

/// `$` is a legal Java identifier character with no other meaning, so
/// metavariables parse as identifiers unchanged.
///
/// The pattern compiler refuses a context with an earlier node of the
/// pattern's kind than the pattern itself, so
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
            "class C {\nvoid m() {\nswitch (_x) {\n@@PATTERN@@\n}\n}\n}\n",
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
        // One `modifiers` node holds the keywords and annotations alike.
        modifiers: Some(colab_rewrite::pattern::Modifiers {
            kinds: &["modifiers"],
        }),
        shorthand_fields: None,
        modifier_tokens: &[],
        parser: None,
    }
}

#[cfg(test)]
mod tests {
    use colab_core::{Error, Operation};
    use colab_rewrite::pattern::PatternAction;

    #[test]
    fn a_switch_case_is_a_pattern() {
        let op = crate::LANG
            .compile_pattern(
                "case HIGH: $$$B",
                PatternAction::Replace("case URGENT: $$$B".into()),
                &[],
            )
            .unwrap();
        let src = "class A {\n  String f(P p) {\n    switch (p) {\n      case HIGH:\n        return \"now\";\n      default:\n        return \"later\";\n    }\n  }\n}\n";
        let out = op.apply(src);
        assert_eq!(out, src.replace("case HIGH", "case URGENT"));
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn calls_are_not_declarations_and_new_ones_are_not_recursion() {
        let src = "class A {\n    int total() { return Integer.valueOf(1) + list.stream().map(A::size).count(); }\n}\n";
        let decls = crate::LANG.declarations(src);
        let names: Vec<&str> = decls[0].children.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["total"], "{decls:?}");
        let op = crate::LANG
            .compile_pattern(
                "new Integer($X)",
                PatternAction::Replace("Integer.valueOf($X)".into()),
                &[],
            )
            .unwrap();
        let before = "class A {\n    Integer f() { return new Integer(1); }\n}\n";
        assert!(op.conflicts(before, &op.apply(before)).is_empty());
    }

    #[test]
    fn a_method_pattern_without_modifiers_matches_any_and_keeps_them() {
        let src = "class A {\n    void a() {}\n    @Override\n    public static void b() {}\n    private final void c() {}\n    int d() { return 1; }\n}\n";
        let any = crate::LANG
            .compile_pattern("void $F() {}", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(any.apply(src), "class A {\n    int d() { return 1; }\n}\n");
        let fill = crate::LANG
            .compile_pattern(
                "void $F() {}",
                PatternAction::Replace("void $F() { run(); }".into()),
                &[],
            )
            .unwrap();
        assert_eq!(
            fill.apply(src),
            "class A {\n    void a() { run(); }\n    @Override\n    public static void b() { run(); }\n    private final void c() { run(); }\n    int d() { return 1; }\n}\n"
        );
        // A template that spells its own modifiers is used as written.
        let hide = crate::LANG
            .compile_pattern(
                "void $F() {}",
                PatternAction::Replace("private void $F() {}".into()),
                &[],
            )
            .unwrap();
        assert_eq!(
            hide.apply("class A {\n    public static void b() {}\n}\n"),
            "class A {\n    private void b() {}\n}\n"
        );
        // A pattern that spells them matches exactly.
        let exact = crate::LANG
            .compile_pattern("public void $F() {}", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(
            exact.apply("class A {\n    void a() {}\n    public void b() {}\n    public static void c() {}\n}\n"),
            "class A {\n    void a() {}\n    public static void c() {}\n}\n"
        );
    }

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
