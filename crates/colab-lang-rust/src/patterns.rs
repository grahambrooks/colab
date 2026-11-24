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
        // The next two let a struct field (`pub conflicts: $T`) and a
        // struct-literal field (`conflicts: $C`) be patterns, e.g. in
        // `where not has '…'`; the last, a match arm
        // (`Some($X) => $E,`).
        contexts: &[
            "@@PATTERN@@;",
            "fn f() {\n@@PATTERN@@\n}\n",
            "struct _S {\n@@PATTERN@@\n}\n",
            "fn f() {\n_S {\n@@PATTERN@@\n};\n}\n",
            "fn f() {\nmatch _m {\n@@PATTERN@@\n}\n}\n",
        ],
        also: &[],
        qualified_calls: Some("::"),
        // `#[derive(…)]` and `///` / `/** */` docs belong to the item after
        // them; `//!` documents the enclosing module, so it stays.
        leading: colab_rewrite::pattern::Leading {
            kinds: &["attribute_item"],
            doc_comments: &["///", "/**"],
        },
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
    fn a_deleted_item_takes_its_docs_and_attributes() {
        let op = crate::LANG
            .compile_pattern("pub struct $S($$$F);", PatternAction::Delete, &[])
            .unwrap();
        let src = "//! The module.\n\nuse x::y;\n\n// About the next two.\n/// A pattern op.\n#[derive(Debug)]\n/** More. */\npub struct PatternOp(u8);\n\n/// Kept.\npub fn f() {}\n";
        assert_eq!(
            op.apply(src),
            "//! The module.\n\nuse x::y;\n\n// About the next two.\n\n/// Kept.\npub fn f() {}\n"
        );
        // A blank line ends what belongs to the item.
        let apart = "/// Orphaned docs.\n\n#[derive(Debug)]\npub struct A(u8);\n";
        assert_eq!(op.apply(apart), "/// Orphaned docs.\n\n");
    }

    #[test]
    fn deleting_items_leaves_one_blank_line_between_neighbours() {
        let op = crate::LANG
            .compile_pattern("fn $F() {}", PatternAction::Delete, &[])
            .unwrap();
        let src = "use a;\n\nfn one() {}\n\nfn two() {}\n\nconst K: u8 = 1;\n";
        assert_eq!(op.apply(src), "use a;\n\nconst K: u8 = 1;\n");
        // At the top of a file, the blank line after it goes too.
        assert_eq!(
            op.apply("fn one() {}\n\nconst K: u8 = 1;\n"),
            "const K: u8 = 1;\n"
        );
        // Between lines with no blank around them, nothing else moves.
        assert_eq!(
            op.apply("const A: u8 = 1;\nfn one() {}\nconst B: u8 = 2;\n"),
            "const A: u8 = 1;\nconst B: u8 = 2;\n"
        );
    }

    #[test]
    fn a_match_arm_is_a_pattern() {
        let op = crate::LANG
            .compile_pattern(
                "(\"symbol\", RuleSpec::Replace { $$$F }) => $E,",
                PatternAction::Delete,
                &[],
            )
            .unwrap();
        let src = "fn b(m: &str, s: RuleSpec) -> R {\n    match (m, s) {\n        (\"import\", RuleSpec::Delete { target }) => del(target),\n        (\"symbol\", RuleSpec::Replace { target, replacement }) => {\n            rename(target, replacement)\n        }\n        (\"symbol\", RuleSpec::Replace { target, replacement }) => rename(target, replacement),\n        (_, spec) => other(spec),\n    }\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        let out = op.apply(src);
        assert_eq!(
            out,
            "fn b(m: &str, s: RuleSpec) -> R {\n    match (m, s) {\n        (\"import\", RuleSpec::Delete { target }) => del(target),\n        (_, spec) => other(spec),\n    }\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        // The last arm, which needs no comma, matches a pattern written with one.
        let last = "fn b(m: u8) -> u8 {\n    match m {\n        0 => 1,\n        (\"symbol\", RuleSpec::Replace { t }) => t\n    }\n}\n";
        assert_eq!(op.locate(last).unwrap().len(), 1);
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
    fn a_bare_sequence_is_a_placeholder_in_an_item_list() {
        let op = crate::LANG
            .compile_pattern(
                "f($X)",
                PatternAction::Replace("g($X)".into()),
                &[colab_core::Constraint {
                    negated: false,
                    test: colab_core::Test::Inside("mod tests { $$$ }".into()),
                }],
            )
            .unwrap();
        let src = "mod tests {\n    fn t() { f(1) }\n}\nfn g() { f(2) }\n";
        assert_eq!(
            op.apply(src),
            "mod tests {\n    fn t() { g(1) }\n}\nfn g() { f(2) }\n"
        );
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
