//! `cpp::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies C++'s grammar, expando character, and the contexts that
//! let a fragment that is not a valid C++ *file* on its own parse.

use colab_rewrite::pattern::PatternLanguage;

/// `$` can appear in C++ identifiers (a compiler extension), so
/// metavariables are parsed as `µ`.
///
/// The pattern compiler refuses a context with an earlier node of the
/// pattern's kind than the pattern itself, so
/// the first context, a top-level expression statement, has nothing before
/// the slot; the function-body contexts, for statements that need one,
/// follow (as written, then with `;` appended). Last, a parameter
/// (`const Graph &$G`); its `typedef` puts a type name before the slot, so
/// a bare name is never also read as a parameter's type.
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        syntax: colab_rewrite::pattern::Syntax::Own(&colab_parse_c::CPP_PARSER),
        expando: 'µ',
        contexts: &[
            "@@PATTERN@@;",
            "void _() {\n@@PATTERN@@\n}\n",
            "void _() {\n@@PATTERN@@;\n}\n",
            "typedef int _t;\nvoid _(@@PATTERN@@);\n",
            "struct _S {\n@@PATTERN@@\n};\n",
        ],
        also: &[],
        // `ns::f(…)` is a call of `f` named by its namespace.
        qualified_calls: Some("::"),
        // Doxygen comments document the declaration after them.
        leading: colab_rewrite::pattern::Leading {
            kinds: &[],
            doc_comments: &["/**", "///"],
        },
        macro_contents: None,
        // A node per specifier (`static`, `inline`, `constexpr`, `const`,
        // `[[nodiscard]]`, `__declspec(…)`), each a child of the item.
        modifiers: Some(colab_rewrite::pattern::Modifiers {
            kinds: &[
                "attribute_declaration",
                "attribute_specifier",
                "storage_class_specifier",
                "type_qualifier",
                "ms_declspec_modifier",
                "explicit_function_specifier",
                "virtual",
            ],
        }),
        shorthand_fields: None,
        modifier_tokens: &[],
        method_calls: None,
    }
}

#[cfg(test)]
mod tests {
    use colab_core::{Error, Operation};
    use colab_rewrite::pattern::PatternAction;

    fn replace(pattern: &str, template: &str, src: &str) -> String {
        let op = crate::LANG
            .compile_pattern(pattern, PatternAction::Replace(template.into()), &[])
            .unwrap();
        let out = op.apply(src);
        assert_eq!(op.apply(&out), out, "idempotent");
        out
    }

    #[test]
    fn a_parameter_and_a_field_are_patterns() {
        let src = "void visit(Id id, int depth);\n";
        assert_eq!(
            replace("Id $I", "NodeId $I", src),
            "void visit(NodeId id, int depth);\n"
        );
        let src = "class Graph {\n    std::vector<Edge> edges_;\n};\n";
        assert_eq!(
            replace("std::vector<Edge> $F;", "std::deque<Edge> $F;", src),
            "class Graph {\n    std::deque<Edge> edges_;\n};\n"
        );
        // The parameter context does not make a bare name a type as well.
        let op = crate::LANG
            .compile_pattern("Id", PatternAction::Delete, &[])
            .unwrap();
        assert!(op.locate("void visit(Id id);\n").unwrap().is_empty());
    }

    #[test]
    fn a_bare_call_pattern_also_finds_namespaced_calls() {
        let op = crate::LANG
            .compile_pattern("old($A)", PatternAction::Replace("next($A)".into()), &[])
            .unwrap();
        let src = "void f() {\n    old(1);\n    m::old(2);\n    ::m::old(3);\n    x.old(4);\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "void f() {\n    next(1);\n    m::next(2);\n    ::m::next(3);\n    x.old(4);\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn rewrites_call_with_two_captures() {
        let op = crate::LANG
            .compile_pattern(
                "std::make_pair($A, $B)",
                PatternAction::Replace("std::pair($A, $B)".into()),
                &[],
            )
            .unwrap();
        let src = "auto f(int k, int v) {\n    return std::make_pair(k, v);\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "auto f(int k, int v) {\n    return std::pair(k, v);\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "cpp::pattern 'std::make_pair($A, $B)' -> 'std::pair($A, $B)'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = crate::LANG
            .compile_pattern(
                "std::printf($$$ARGS)",
                PatternAction::Replace("fmt::print($$$ARGS)".into()),
                &[],
            )
            .unwrap();
        let src = "void f(int n) {\n    std::printf(\"%d\\n\", n);\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "void f(int n) {\n    fmt::print(\"%d\\n\", n);\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = crate::LANG
            .compile_pattern("DEBUG_TRACE($X)", PatternAction::Delete, &[])
            .unwrap();
        let src = "void f(int x) {\n    DEBUG_TRACE(x);\n    use(x);\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "void f(int x) {\n    use(x);\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "void f() {\n    int y = foo.bar(1, 2);\n    y = 3;\n}\n";
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
        let src = "void f() {\n    int y = legacy_name.bar(1, 2);\n    y = 3;\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = crate::LANG
            .compile_pattern("void (", PatternAction::Delete, &[])
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("cpp::pattern"), "{err}");
    }
}
