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
        // `where not has '…'`; then a match arm (`Some($X) => $E,`); the
        // last, a type on its own (`HashMap<PathBuf, String>`), which no
        // other context parses.
        contexts: &[
            "@@PATTERN@@;",
            "fn f() {\n@@PATTERN@@\n}\n",
            "struct _S {\n@@PATTERN@@\n}\n",
            "fn f() {\n_S {\n@@PATTERN@@\n};\n}\n",
            "fn f() {\nmatch _m {\n@@PATTERN@@\n}\n}\n",
            "type _T = @@PATTERN@@;\n",
        ],
        also: &[],
        qualified_calls: Some("::"),
        // `#[derive(…)]` and `///` / `/** */` docs belong to the item after
        // them; `//!` documents the enclosing module, so it stays.
        leading: colab_rewrite::pattern::Leading {
            kinds: &["attribute_item"],
            doc_comments: &["///", "/**"],
        },
        // A macro call's arguments are a token tree to tree-sitter. These
        // macros take expressions, so their contents are read as call
        // arguments; any other macro (`matches!`'s pattern, a DSL such as
        // `html!` or `sqlx::query!`) is left alone.
        macro_contents: Some(colab_rewrite::pattern::MacroContents {
            invocation: "macro_invocation",
            name_field: "macro",
            body: "token_tree",
            macros: EXPRESSION_MACROS,
            wrapper: "fn _w() {\n_m(@@PATTERN@@);\n}\n",
        }),
        modifiers: Some(colab_rewrite::pattern::Modifiers {
            sentinel: "pub(in colab_mods)",
            kinds: &["visibility_modifier"],
            several: false,
        }),
    }
}

/// Macros whose arguments are comma-separated expressions: the standard
/// library's, and the common logging and error crates'.
const EXPRESSION_MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "vec",
    "format",
    "format_args",
    "print",
    "println",
    "eprint",
    "eprintln",
    "write",
    "writeln",
    "panic",
    "todo",
    "unimplemented",
    "unreachable",
    "dbg",
    "anyhow",
    "bail",
    "ensure",
    "trace",
    "debug",
    "info",
    "warn",
    "error",
    // serde_json: an object literal does not parse as arguments, so only
    // `json!(expr)` and arrays are read.
    "json",
];

#[cfg(test)]
mod tests {
    use colab_core::{Error, Operation};
    use colab_rewrite::pattern::PatternAction;

    #[test]
    fn an_item_pattern_without_a_visibility_matches_any_and_keeps_it() {
        let src = "fn a() {}\npub fn b() {}\npub(super) fn c() {}\npub(crate) struct S;\n";
        // A delete finds every visibility.
        let any = crate::LANG
            .compile_pattern("fn $F() {}", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(any.apply(src), "pub(crate) struct S;\n");
        // A rename keeps the visibility each item had.
        let rename = crate::LANG
            .compile_pattern(
                "fn $F() {}",
                PatternAction::Replace("fn $F() { todo!() }".into()),
                &[],
            )
            .unwrap();
        assert_eq!(
            rename.apply(src),
            "fn a() { todo!() }\npub fn b() { todo!() }\npub(super) fn c() { todo!() }\npub(crate) struct S;\n"
        );
        // A template that spells its own visibility is used as written.
        let publish = crate::LANG
            .compile_pattern(
                "fn $F() {}",
                PatternAction::Replace("pub(crate) fn $F() {}".into()),
                &[],
            )
            .unwrap();
        assert_eq!(
            publish.apply(src),
            "pub(crate) fn a() {}\npub(crate) fn b() {}\npub(crate) fn c() {}\npub(crate) struct S;\n"
        );
        // A pattern that spells one matches exactly.
        let exact = crate::LANG
            .compile_pattern("pub fn $F() {}", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(
            exact.apply(src),
            "fn a() {}\npub(super) fn c() {}\npub(crate) struct S;\n"
        );
        // A struct pattern too, and a non-item pattern gains no forms.
        let structs = crate::LANG
            .compile_pattern("struct $S;", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(
            structs.apply(src),
            "fn a() {}\npub fn b() {}\npub(super) fn c() {}\n"
        );
        let call = crate::LANG
            .compile_pattern("y()", PatternAction::Replace("z()".into()), &[])
            .unwrap();
        assert_eq!(call.apply("pub fn x() { y(); }\n"), "pub fn x() { z(); }\n");
    }

    #[test]
    fn a_type_alone_is_a_pattern() {
        let op = crate::LANG
            .compile_pattern(
                "HashMap<PathBuf, $V>",
                PatternAction::Replace("BTreeMap<PathBuf, $V>".into()),
                &[],
            )
            .unwrap();
        let src = "fn f(m: HashMap<PathBuf, String>) -> HashMap<PathBuf, u8> {\n    let x: HashMap<PathBuf, ()> = g();\n    todo!()\n}\n";
        assert_eq!(
            op.apply(src),
            "fn f(m: BTreeMap<PathBuf, String>) -> BTreeMap<PathBuf, u8> {\n    let x: BTreeMap<PathBuf, ()> = g();\n    todo!()\n}\n"
        );
        // Either leaf literal is a sound prefilter.
        assert!(matches!(op.prefilter(), Some("HashMap" | "PathBuf")));
    }

    #[test]
    fn a_struct_literal_pattern_matches_the_type_named_by_its_path_too() {
        let op = crate::LANG
            .compile_pattern(
                "FileChange { $$$F conflicts: $C, }",
                PatternAction::Replace("FileChange { $$$F conflicts: $C, created: false, }".into()),
                &[],
            )
            .unwrap();
        let src = "fn f() {\n    let a = FileChange { path, conflicts: c, };\n    let b = crate::walker::FileChange { path, conflicts: c, };\n}\n";
        assert_eq!(
            op.apply(src),
            "fn f() {\n    let a = FileChange { path, conflicts: c, created: false, };\n    let b = crate::walker::FileChange { path, conflicts: c, created: false, };\n}\n"
        );
        // A keyword's block is never a struct literal.
        let block = crate::LANG
            .compile_pattern("unsafe { $$$B }", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(
            block.apply("fn f() {\n    unsafe { g() }\n}\n"),
            "fn f() {\n}\n"
        );
    }

    #[test]
    fn json_macro_arguments_are_read_when_they_are_expressions() {
        let op = crate::LANG
            .compile_pattern("old($A)", PatternAction::Replace("new($A)".into()), &[])
            .unwrap();
        // An object literal is not an expression list: left alone.
        let src =
            "fn f() {\n    json!(old(1));\n    json!([old(2)]);\n    json!({\"k\": old(3)});\n}\n";
        assert_eq!(
            op.apply(src),
            "fn f() {\n    json!(new(1));\n    json!([new(2)]);\n    json!({\"k\": old(3)});\n}\n"
        );
    }

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
    fn a_skipped_rewrite_names_the_one_match_that_breaks_the_file() {
        // The dogfood case: of several literals, one ends in `..base`,
        // after which the template's new field cannot follow.
        let op = crate::LANG
            .compile_pattern(
                "PatternLanguage { $$$F }",
                PatternAction::Replace("PatternLanguage { $$$F leading: None, }".into()),
                &[],
            )
            .unwrap();
        let src = "fn a() -> PatternLanguage {\n    PatternLanguage { grammar: g, }\n}\nfn b() -> PatternLanguage {\n    PatternLanguage { grammar: h, ..rust() }\n}\n";
        assert_eq!(op.apply(src), src, "skipped as a whole");
        let why = op.why_unchanged(src).unwrap();
        assert!(why.contains("at line 5, column 5"), "{why}");
    }

    #[test]
    fn captures_keep_their_own_layout_when_a_template_moves_them() {
        // Swapped, a one-line capture stays on one line and a many-line
        // one keeps its lines, each where the template puts it.
        let swap = crate::LANG
            .compile_pattern(
                "swap($A, $B)",
                PatternAction::Replace("swapped($B, $A)".into()),
                &[],
            )
            .unwrap();
        let src = "fn main() {\n    swap(\n        f(\n            a,\n            b,\n        ),\n        f(a, b),\n    );\n}\n";
        assert_eq!(
            swap.apply(src),
            "fn main() {\n    swapped(\n        f(a, b),\n        f(\n            a,\n            b,\n        )\n    );\n}\n"
        );
        // An item the template appends after `$$$B` starts its own line.
        let append = crate::LANG
            .compile_pattern(
                "impl L for R { $$$B }",
                PatternAction::Replace("impl L for R { $$$B fn three(&self) -> u8 { 3 } }".into()),
                &[],
            )
            .unwrap();
        let src = "impl L for R {\n    fn two(&self) -> u8 {\n        2\n    }\n}\n";
        let out = append.apply(src);
        assert_eq!(
            out,
            "impl L for R {\n    fn two(&self) -> u8 {\n        2\n    }\n    fn three(&self) -> u8 { 3 }\n}\n"
        );
        // A prefix moves the capture, not its body: `unsafe $X`.
        let prefix = crate::LANG
            .compile_pattern(
                "$X",
                PatternAction::Replace("unsafe $X".into()),
                &[colab_core::Constraint {
                    negated: false,
                    test: colab_core::Test::Is {
                        var: colab_core::Var {
                            name: "X".into(),
                            multi: false,
                        },
                        kind: "foreign_mod_item".into(),
                    },
                }],
            )
            .unwrap();
        assert_eq!(
            prefix.apply("extern \"C\" {\n    fn abs(x: i32) -> i32;\n}\n"),
            "unsafe extern \"C\" {\n    fn abs(x: i32) -> i32;\n}\n"
        );
    }

    #[test]
    fn patterns_read_inside_expression_macros() {
        let src = "fn t(p: &Path, x: Option<u8>) {\n    assert!(is_relevant(p));\n    let v = vec![a, is_relevant(p), vec![is_relevant(q)]];\n    let m = matches!(x, Some(_));\n    let r = vec![is_relevant(p); 3];\n    custom!(is_relevant(p));\n}\n";
        let find = crate::LANG
            .compile_pattern("is_relevant($P)", PatternAction::Delete, &[])
            .unwrap();
        let lines: Vec<usize> = find.locate(src).unwrap().iter().map(|l| l.line).collect();
        // `assert!`, `vec!` and the `vec!` nested in it; not `vec![x; n]`
        // (whose contents are not arguments), nor an unknown macro.
        assert_eq!(lines, vec![2, 3, 3], "{lines:?}");
        let replace = crate::LANG
            .compile_pattern(
                "is_relevant($P)",
                PatternAction::Replace("LANG.is_source($P)".into()),
                &[],
            )
            .unwrap();
        let out = replace.apply(src);
        assert_eq!(
            out,
            "fn t(p: &Path, x: Option<u8>) {\n    assert!(LANG.is_source(p));\n    let v = vec![a, LANG.is_source(p), vec![LANG.is_source(q)]];\n    let m = matches!(x, Some(_));\n    let r = vec![is_relevant(p); 3];\n    custom!(is_relevant(p));\n}\n"
        );
        assert_eq!(replace.apply(&out), out, "idempotent");
        // A delete takes an argument's comma, as in a call.
        let delete = crate::LANG
            .compile_pattern("legacy($X)", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(
            delete.apply("fn f() { let v = vec![a, legacy(1), b]; }\n"),
            "fn f() { let v = vec![a, b]; }\n"
        );
        // `matches!`'s argument is a pattern, never read as an expression.
        let some = crate::LANG
            .compile_pattern("Some($X)", PatternAction::Delete, &[])
            .unwrap();
        assert!(some.locate(src).unwrap().is_empty());
    }

    fn when(negated: bool, test: colab_core::Test) -> colab_core::Constraint {
        colab_core::Constraint { negated, test }
    }

    fn renamed(constraints: &[colab_core::Constraint], src: &str) -> String {
        crate::LANG
            .compile_pattern(
                "is_relevant($P)",
                PatternAction::Replace("LANG.is_source($P)".into()),
                constraints,
            )
            .unwrap()
            .apply(src)
    }

    #[test]
    fn a_replace_that_unwraps_an_item_takes_its_attributes() {
        let unwrap = crate::LANG
            .compile_pattern(
                "mod tests { $$$B }",
                PatternAction::Replace("$$$B".into()),
                &[],
            )
            .unwrap();
        let src = "fn a() {}\n\n/// The tests.\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n";
        assert_eq!(unwrap.apply(src), "fn a() {}\n\nfn t() {}\n");
        // A template that writes the item again keeps what is above it.
        let rename = crate::LANG
            .compile_pattern(
                "fn old() {}",
                PatternAction::Replace("fn new() {}".into()),
                &[],
            )
            .unwrap();
        let attributed = "/// Docs.\n#[inline]\nfn old() {}\n";
        assert_eq!(
            rename.apply(attributed),
            "/// Docs.\n#[inline]\nfn new() {}\n"
        );
    }

    #[test]
    fn top_keeps_a_rule_to_the_files_own_items() {
        use colab_core::Test;
        let src = "use a::b;\nuse c::d;\n\nfn f() {\n    use e::g;\n    h();\n}\n";
        let deleted = |negated| {
            crate::LANG
                .compile_pattern(
                    "use $$$X;",
                    PatternAction::Delete,
                    &[when(negated, Test::Top)],
                )
                .unwrap()
                .apply(src)
        };
        // The file's imports go; the one inside the function stays.
        let top = deleted(false);
        assert!(
            !top.contains("use a::b") && !top.contains("use c::d"),
            "{top}"
        );
        assert!(top.contains("    use e::g;"), "{top}");
        // `not top`: only what is nested.
        let nested = deleted(true);
        assert!(nested.starts_with("use a::b;\nuse c::d;\n"), "{nested}");
        assert!(!nested.contains("use e::g"), "{nested}");
        // A call in a macro's contents is inside the macro, never one of
        // the file's items, even where the macro itself is.
        let in_macro = "println!(\"{}\", f(1));\n";
        let found = |negated| {
            crate::LANG
                .compile_pattern("f($X)", PatternAction::Delete, &[when(negated, Test::Top)])
                .unwrap()
                .locate(in_macro)
                .unwrap()
                .len()
        };
        assert_eq!((found(false), found(true)), (0, 1));
    }

    #[test]
    fn a_match_in_a_macro_is_judged_by_where_the_macro_is() {
        use colab_core::{Relation, Stop, Test};
        let src = "fn t(p: &Path) {\n    assert!(is_relevant(p));\n    let r = is_relevant(p);\n}\nfn u(p: &Path) {\n    assert!(is_relevant(p));\n}\n";
        // `inside` reads the file around the macro, never the wrapper.
        let inside_t = renamed(
            &[when(false, Test::Inside("fn t($$$A) { $$$B }".into()))],
            src,
        );
        assert_eq!(
            inside_t,
            "fn t(p: &Path) {\n    assert!(LANG.is_source(p));\n    let r = LANG.is_source(p);\n}\nfn u(p: &Path) {\n    assert!(is_relevant(p));\n}\n"
        );
        assert_eq!(
            renamed(
                &[when(false, Test::Inside("fn t($$$A) { $$$B }".into()))],
                &inside_t
            ),
            inside_t,
            "idempotent"
        );
        // `not inside` too: the wrapper's `fn _w()` is no function of the file's.
        let not_t = renamed(
            &[when(true, Test::Inside("fn t($$$A) { $$$B }".into()))],
            src,
        );
        assert!(
            not_t.contains("fn u(p: &Path) {\n    assert!(LANG.is_source(p));")
                && not_t.contains("    assert!(is_relevant(p));\n    let r = is_relevant(p);"),
            "{not_t}"
        );
        assert!(
            !renamed(&[when(false, Test::Inside("fn _w() { $$$B }".into()))], src).contains("LANG"),
            "the wrapper is not an ancestor"
        );
        // The macro call itself encloses its arguments, directly.
        let directly = |pattern: &str| {
            renamed(
                &[when(
                    false,
                    Test::Related {
                        relation: Relation::Inside,
                        pattern: pattern.into(),
                        stop: Stop::Directly,
                    },
                )],
                "fn f(p: &Path) {\n    assert!(is_relevant(p));\n    debug_assert!(ok(is_relevant(p)));\n}\n",
            )
        };
        assert_eq!(
            directly("assert!($$$A)"),
            "fn f(p: &Path) {\n    assert!(LANG.is_source(p));\n    debug_assert!(ok(is_relevant(p)));\n}\n"
        );
        // Neighbours and positions are the contents' own.
        let v = "fn f() {\n    let v = vec![a, is_relevant(p), is_relevant(q)];\n}\n";
        let follows_a = renamed(
            &[when(
                false,
                Test::Related {
                    relation: Relation::Follows,
                    pattern: "a".into(),
                    stop: Stop::Directly,
                },
            )],
            v,
        );
        assert_eq!(
            follows_a,
            "fn f() {\n    let v = vec![a, LANG.is_source(p), is_relevant(q)];\n}\n"
        );
        assert_eq!(
            renamed(&[when(false, Test::Nth(-1))], v),
            "fn f() {\n    let v = vec![a, is_relevant(p), LANG.is_source(q)];\n}\n"
        );
    }

    #[test]
    fn a_semantic_question_about_a_macro_argument_is_asked_of_the_file() {
        use std::sync::Arc;
        /// Answers `true` only about the file's own text at `good`.
        struct OnlyTheFile;
        impl colab_core::semantic::Oracle for OnlyTheFile {
            fn ask(
                &self,
                _path: &std::path::Path,
                source: &str,
                span: std::ops::Range<usize>,
                _query: &colab_core::semantic::Query,
            ) -> Result<bool, String> {
                Ok(source.starts_with("fn t(") && &source[span] == "good")
            }
        }
        let typed = [when(
            false,
            colab_core::Test::Semantic {
                var: colab_core::Var {
                    name: "P".into(),
                    multi: false,
                },
                query: colab_core::semantic::Query::Type("Path".into()),
            },
        )];
        let src = "fn t() {\n    assert!(is_relevant(good));\n    assert!(is_relevant(bad));\n    let r = is_relevant(good);\n}\n";
        let run = colab_core::run::Run::new();
        let out = run.enter(|| {
            colab_core::semantic::at_path(std::path::Path::new("t.rs"), || {
                colab_core::semantic::install(Arc::new(OnlyTheFile));
                let out = renamed(&typed, src);
                colab_core::semantic::uninstall();
                out
            })
        });
        assert_eq!(
            out,
            "fn t() {\n    assert!(LANG.is_source(good));\n    assert!(is_relevant(bad));\n    let r = LANG.is_source(good);\n}\n"
        );
        assert!(run.take_failure().is_none());
    }

    #[test]
    fn a_project_lists_its_own_expression_macros() {
        let src = "fn t() {\n    my_assert!(is_relevant(p));\n}\n";
        assert_eq!(renamed(&[], src), src, "an unknown macro is not read");
        let run = colab_core::run::Run::new();
        run.set_macros(vec!["my_assert".into()]);
        assert_eq!(
            run.enter(|| renamed(&[], src)),
            "fn t() {\n    my_assert!(LANG.is_source(p));\n}\n"
        );
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
