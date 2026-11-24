//! `csharp::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies C#'s grammar, expando character, and the contexts that
//! let a fragment that is not a valid C# *file* on its own parse.

use colab_rewrite::pattern::PatternLanguage;

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
        also: &[],
        qualified_calls: None,
        // XML doc comments document the member after them; attributes are
        // already part of the declaration.
        leading: colab_rewrite::pattern::Leading {
            kinds: &[],
            doc_comments: &["///", "/**"],
        },
    }
}

#[cfg(test)]
mod tests {
    use colab_core::{Error, Operation};
    use colab_rewrite::pattern::PatternAction;

    #[test]
    fn a_deleted_method_takes_its_xml_docs() {
        let op = crate::LANG
            .compile_pattern("class $C {}", PatternAction::Delete, &[])
            .unwrap();
        let src = "// a note\n/// <summary>Old.</summary>\n/// <remarks>Gone.</remarks>\nclass Old {}\nclass Kept { int K() { return 1; } }\n";
        assert_eq!(
            op.apply(src),
            "// a note\nclass Kept { int K() { return 1; } }\n"
        );
    }

    #[test]
    fn rewrites_call_with_two_captures() {
        let op = crate::LANG
            .compile_pattern(
                "Assert.AreEqual($A, $B)",
                PatternAction::Replace("Assert.That($B, Is.EqualTo($A))".into()),
                &[],
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
        let op = crate::LANG
            .compile_pattern(
                "Console.WriteLine($$$ARGS)",
                PatternAction::Replace("logger.LogInformation($$$ARGS)".into()),
                &[],
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
        let op = crate::LANG
            .compile_pattern("Debug.WriteLine($X)", PatternAction::Delete, &[])
            .unwrap();
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
        let src = "class C {\n    void M() {\n        var y = legacy_name.Bar(1, 2);\n        y = 3;\n    }\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn a_method_pattern_matches_class_members() {
        // Bare, `void $F() {}` is a top-level local function; it is
        // matched as a class member too.
        let op = crate::LANG
            .compile_pattern("void $F() {}", PatternAction::Delete, &[])
            .unwrap();
        let src = "class K {\n    void Old() {}\n    int Keep() { return 1; }\n}\n";
        assert_eq!(op.locate(src).unwrap().len(), 1);
        assert_eq!(
            op.apply(src),
            "class K {\n    int Keep() { return 1; }\n}\n"
        );
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = crate::LANG
            .compile_pattern("class (", PatternAction::Delete, &[])
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("csharp::pattern"), "{err}");
    }
}
