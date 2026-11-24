//! `kotlin::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Kotlin's grammar, expando character, and the contexts that
//! let a fragment that is not a valid Kotlin *file* on its own parse.

use colab_rewrite::pattern::PatternLanguage;

/// `$` starts a string template in Kotlin, so metavariables are parsed as
/// `µ`. Contexts, in order: a statement or expression inside a function
/// body, a class member, then a `when` entry (`1 -> $E`).
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_kotlin_ng::LANGUAGE.into(),
        expando: 'µ',
        contexts: &[
            "fun f() {\n@@PATTERN@@\n}\n",
            "class C {\n@@PATTERN@@\n}\n",
            "fun f() {\nwhen (_x) {\n@@PATTERN@@\n}\n}\n",
        ],
        also: &[],
        qualified_calls: None,
        // KDoc documents the declaration after it.
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
    fn a_deleted_function_takes_its_kdoc() {
        let op = crate::LANG
            .compile_pattern("fun $F() {}", PatternAction::Delete, &[])
            .unwrap();
        let src = "// a note\n/** Old. */\nfun old() {}\nfun kept() = 1\n";
        assert_eq!(op.apply(src), "// a note\nfun kept() = 1\n");
    }

    #[test]
    fn rewrites_call_with_two_captures() {
        let op = crate::LANG
            .compile_pattern(
                "Log.d($TAG, $MSG)",
                PatternAction::Replace("Timber.tag($TAG).d($MSG)".into()),
                &[],
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
        let op = crate::LANG
            .compile_pattern(
                "println($$$ARGS)",
                PatternAction::Replace("logger.info($$$ARGS)".into()),
                &[],
            )
            .unwrap();
        let src = "fun main() {\n    println(\"hello\")\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "fun main() {\n    logger.info(\"hello\")\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = crate::LANG
            .compile_pattern("println($$$ARGS)", PatternAction::Delete, &[])
            .unwrap();
        let src = "fun main() {\n    val x = 1\n    println(x)\n    use(x)\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "fun main() {\n    val x = 1\n    use(x)\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "fun f(): Int {\n    val y = foo.bar(1, 2)\n    return y\n}\n";
        for pattern in ["foo.bar($A, $B)", "return $X"] {
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
        let src = "fun f(): Int {\n    val y = legacy_name.bar(1, 2)\n    return y\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn a_when_entry_is_a_pattern() {
        let op = crate::LANG
            .compile_pattern(
                "1 -> $E",
                PatternAction::Replace("1 -> other()".into()),
                &[],
            )
            .unwrap();
        let src = "fun f(x: Int) = when (x) {\n    1 -> g()\n    2 -> h()\n    else -> k()\n}\n";
        assert_eq!(
            op.apply(src),
            "fun f(x: Int) = when (x) {\n    1 -> other()\n    2 -> h()\n    else -> k()\n}\n"
        );
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = crate::LANG
            .compile_pattern("fun (", PatternAction::Delete, &[])
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("kotlin::pattern"), "{err}");
    }
}
