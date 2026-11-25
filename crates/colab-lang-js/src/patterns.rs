//! `js::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies JavaScript's grammar, expando character, and the contexts that
//! let a fragment that is not a valid JavaScript *file* on its own parse.

use colab_rewrite::pattern::{PatternLanguage, Qualifiers};

/// `$` is a legal JavaScript identifier character, so metavariables parse
/// as identifiers unchanged. Expressions and statements parse on their own;
/// the contexts admit a class member such as a method, and a `switch`
/// case (`case $X: $$$B`).
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        syntax: colab_rewrite::pattern::Syntax::TreeSitter(tree_sitter_javascript::LANGUAGE.into()),
        expando: '$',
        contexts: &[
            "class C {\n@@PATTERN@@\n}\n",
            "switch (_x) {\n@@PATTERN@@\n}\n",
        ],
        // `debug: true` alone is a labelled statement; in an object it is
        // the property a pattern usually means. Match both.
        also: &["({@@PATTERN@@});\n"],
        qualified_calls: None,
        // JSDoc documents the declaration after it.
        leading: colab_rewrite::pattern::Leading {
            kinds: &[],
            doc_comments: &["/**"],
        },
        macro_contents: None,
        modifiers: None,
        shorthand_fields: Some(colab_rewrite::pattern::Shorthand::BRACES),
        // A method's modifiers are anonymous tokens: kept by a rewrite.
        modifier_tokens: &["static", "async", "get", "set", "*", "accessor"],
    }
}

/// A rewrite that introduces `x.` with no import or `require` binding `x`
/// is a conflict (see [`Qualifiers`]). Globals need no import; a name the
/// file already uses anywhere (a local, a parameter) counts as known.
pub(crate) const QUALIFIERS: Qualifiers = Qualifiers {
    kind: "member_expression",
    field: "object",
    imported: crate::imports::imported_names,
    builtin: GLOBALS,
};

/// The globals a qualified reference needs no import for, in JavaScript
/// and TypeScript alike.
pub const GLOBALS: &[&str] = &[
    "Array",
    "Atomics",
    "BigInt",
    "Boolean",
    "Buffer",
    "DataView",
    "Date",
    "Error",
    "Function",
    "Intl",
    "JSON",
    "Map",
    "Math",
    "Number",
    "Object",
    "Promise",
    "Proxy",
    "Reflect",
    "RegExp",
    "Set",
    "String",
    "Symbol",
    "URL",
    "URLSearchParams",
    "WeakMap",
    "WeakSet",
    "console",
    "crypto",
    "document",
    "exports",
    "fetch",
    "global",
    "globalThis",
    "localStorage",
    "location",
    "module",
    "navigator",
    "performance",
    "process",
    "require",
    "self",
    "sessionStorage",
    "super",
    "this",
    "window",
];

#[cfg(test)]
mod tests {
    use colab_core::{Error, Operation};
    use colab_rewrite::pattern::PatternAction;

    #[test]
    fn an_object_field_pattern_matches_its_shorthand() {
        let op = crate::LANG
            .compile_pattern(
                "connect({ host: $H, port: $P })",
                PatternAction::Replace("open({ host: $H, port: $P })".into()),
                &[],
            )
            .unwrap();
        let src = "connect({ host, port: 1 });\nconnect({ host: h, port });\n";
        assert_eq!(
            op.apply(src),
            "open({ host, port: 1 });\nopen({ host: h, port });\n"
        );
    }

    #[test]
    fn a_rewrite_keeps_static_and_async() {
        // Matching skipped the anonymous `static async`, and the rewrite
        // replaced the whole method, so they were silently dropped.
        let op = crate::LANG
            .compile_pattern("f() {}", PatternAction::Replace("f2() {}".into()), &[])
            .unwrap();
        assert_eq!(
            op.apply("class A {\n  static async f() {}\n  f() {}\n}\n"),
            "class A {\n  static async f2() {}\n  f2() {}\n}\n"
        );
        // A pattern that spells one is exact about it, and its template's
        // choice stands.
        let spelled = crate::LANG
            .compile_pattern(
                "static f() {}",
                PatternAction::Replace("f() {}".into()),
                &[],
            )
            .unwrap();
        assert_eq!(
            spelled.apply("class A {\n  static f() {}\n}\n"),
            "class A {\n  f() {}\n}\n"
        );
    }

    #[test]
    fn a_deleted_declaration_takes_its_jsdoc() {
        let op = crate::LANG
            .compile_pattern("function $F() {}", PatternAction::Delete, &[])
            .unwrap();
        let src =
            "// helpers\n/**\n * Old.\n */\nfunction old() {}\nfunction kept() { return 1; }\n";
        assert_eq!(op.apply(src), "// helpers\nfunction kept() { return 1; }\n");
    }

    #[test]
    fn rewrites_call_with_two_captures() {
        let op = crate::LANG
            .compile_pattern(
                "$A.indexOf($B) !== -1",
                PatternAction::Replace("$A.includes($B)".into()),
                &[],
            )
            .unwrap();
        let src = "if (names.indexOf(name) !== -1) {\n  greet(name);\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "if (names.includes(name)) {\n  greet(name);\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "js::pattern '$A.indexOf($B) !== -1' -> '$A.includes($B)'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = crate::LANG
            .compile_pattern(
                "console.log($$$ARGS)",
                PatternAction::Replace("logger.info($$$ARGS)".into()),
                &[],
            )
            .unwrap();
        let src = "function f(a, b) {\n  console.log('sum', a + b);\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "function f(a, b) {\n  logger.info('sum', a + b);\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = crate::LANG
            .compile_pattern("console.debug($$$ARGS)", PatternAction::Delete, &[])
            .unwrap();
        let src = "function f(x) {\n  console.debug('x =', x);\n  return x;\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "function f(x) {\n  return x;\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "function f() {\n  let y = foo.bar(1, 2);\n  y = 3;\n}\n";
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
        let src = "function f() {\n  let y = legacy_name.bar(1, 2);\n  y = 3;\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn a_switch_case_is_a_pattern() {
        let op = crate::LANG
            .compile_pattern("case 1: $$$B", PatternAction::Delete, &[])
            .unwrap();
        let src = "function f(x) {\n  switch (x) {\n    case 1:\n      g();\n      break;\n    case 2:\n      h();\n  }\n}\n";
        assert_eq!(
            op.apply(src),
            "function f(x) {\n  switch (x) {\n    case 2:\n      h();\n  }\n}\n"
        );
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = crate::LANG
            .compile_pattern("function (", PatternAction::Delete, &[])
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("js::pattern"), "{err}");
    }
}
