//! `rust::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Rust's grammar, expando character, and the contexts that
//! let a fragment that is not a valid Rust *file* on its own parse.

use colab_rewrite::pattern::PatternLanguage;

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
        // The last two let a struct field (`pub conflicts: $T`) and a
        // struct-literal field (`conflicts: $C`) be patterns, e.g. in
        // `where not has '…'`.
        contexts: &[
            "@@PATTERN@@;",
            "fn f() {\n@@PATTERN@@\n}\n",
            "struct _S {\n@@PATTERN@@\n}\n",
            "fn f() {\n_S {\n@@PATTERN@@\n};\n}\n",
        ],
        also: &[],
        qualified_calls: Some("::"),
    }
}

#[cfg(test)]
mod tests {
    use colab_core::{Error, Operation};
    use colab_rewrite::pattern::PatternAction;

    #[test]
    fn struct_and_literal_fields_are_patterns() {
        let decl = crate::LANG
            .compile_pattern("pub conflicts: $T", PatternAction::Delete, &[])
            .unwrap();
        let src = "struct A {\n    pub path: String,\n    pub conflicts: Vec<u8>,\n}\n";
        assert_eq!(decl.apply(src), "struct A {\n    pub path: String,\n}\n");
        // A literal field, as a `where not has` constraint would use it.
        let lit = crate::LANG
            .compile_pattern(
                "FileChange { $$$F }",
                PatternAction::Replace("FileChange { $$$F conflicts: Vec::new() }".into()),
                &[colab_core::Constraint {
                    negated: true,
                    test: colab_core::Test::Has("conflicts: $C".into()),
                }],
            )
            .unwrap();
        let done = "fn f() { let c = FileChange { path: p, conflicts: x }; }\n";
        assert_eq!(lit.apply(done), done, "already has the field");
    }

    #[test]
    fn a_field_pattern_matches_declarations_and_literals() {
        let op = crate::LANG
            .compile_pattern("also: $A", PatternAction::Delete, &[])
            .unwrap();
        let src = "struct L {\n    also: Vec<u8>,\n    name: u8,\n}\n\nfn f() -> L {\n    L {\n        also: Vec::new(),\n        name: 1,\n    }\n}\n";
        let found = op.locate(src).unwrap();
        let texts: Vec<&str> = found.iter().map(|f| f.text.as_str()).collect();
        assert_eq!(texts, ["also: Vec<u8>", "also: Vec::new()"]);
        assert_eq!(op.prefilter(), Some("also"));
        assert_eq!(
            op.apply(src),
            "struct L {\n    name: u8,\n}\n\nfn f() -> L {\n    L {\n        name: 1,\n    }\n}\n"
        );
    }

    #[test]
    fn a_struct_literal_is_rewritten_to_an_expression() {
        let op = crate::LANG
            .compile_pattern(
                "SymbolRename { from: $F.into(), to: $T.into() }",
                PatternAction::Replace("crate::LANG.symbol_rename($F, $T)".into()),
                &[],
            )
            .unwrap();
        let src = "fn f() -> R {\n    let op = SymbolRename { from: a.into(), to: b.into() };\n    Ok(Box::new(SymbolRename { from: c.into(), to: d.into() }))\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "fn f() -> R {\n    let op = crate::LANG.symbol_rename(a, b);\n    Ok(Box::new(crate::LANG.symbol_rename(c, d)))\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn rewrites_call_with_two_captures() {
        let op = crate::LANG
            .compile_pattern(
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
        let op = crate::LANG
            .compile_pattern(
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
        let op = crate::LANG
            .compile_pattern("dbg!($X)", PatternAction::Delete, &[])
            .unwrap();
        let src = "fn f(x: u32) -> u32 {\n    dbg!(x);\n    x + 1\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "fn f(x: u32) -> u32 {\n    x + 1\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "fn f() {\n    let y = foo.bar(1, 2);\n    y\n}\n";
        for pattern in ["foo.bar($A, $B)", "let $X = $Y;"] {
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
        let src = "fn f() {\n    let y = legacy_name.bar(1, 2);\n    y\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = crate::LANG
            .compile_pattern("fn (", PatternAction::Delete, &[])
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("rust::pattern"), "{err}");
    }
}
