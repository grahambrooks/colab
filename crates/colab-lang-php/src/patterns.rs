//! `php::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies PHP's grammar, expando character, and the contexts that
//! let a fragment that is not a valid PHP *file* on its own parse.

use colab_rewrite::pattern::PatternLanguage;

/// `$x` is a PHP variable, so metavariables are `$UPPER` and parse as `µ`
/// (a PHP name); a lower-case `$x` stays a variable.
///
/// This uses the PHP-only grammar, not the HTML-embedding one the other
/// `php::` operations use: under that grammar any text outside `<?php` is
/// inline HTML, so a bare pattern would "parse" as one opaque text node. The
/// PHP-only grammar still accepts a leading `<?php` tag, so ordinary files
/// match normally; only inline HTML around the code is seen as an error
/// region (matches elsewhere in the file are unaffected). Contexts, in
/// order: an expression made a statement with `;`, a class member, a
/// `match` arm (`Status::Draft => $V`), a `switch` case, and the list an
/// `echo` prints (`$A, "\n"`).
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_php::LANGUAGE_PHP_ONLY.into(),
        expando: 'µ',
        contexts: &[
            "@@PATTERN@@;",
            "class C {\n@@PATTERN@@\n}\n",
            "match ($_x) {\n@@PATTERN@@\n};\n",
            "switch ($_x) {\n@@PATTERN@@\n}\n",
            "echo @@PATTERN@@;\n",
        ],
        also: &[],
        // `\M\f(…)` is a call of `f` named by its namespace.
        qualified_calls: Some("\\"),
        // PHPDoc documents the declaration after it.
        leading: colab_rewrite::pattern::Leading {
            kinds: &[],
            doc_comments: &["/**"],
        },
        macro_contents: None,
        // A node per keyword, and the attribute list before them.
        modifiers: Some(colab_rewrite::pattern::Modifiers {
            kinds: &[
                "attribute_list",
                "visibility_modifier",
                "static_modifier",
                "final_modifier",
                "abstract_modifier",
                "readonly_modifier",
                "var_modifier",
            ],
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

    fn replace(pattern: &str, template: &str, src: &str) -> String {
        let op = crate::LANG
            .compile_pattern(pattern, PatternAction::Replace(template.into()), &[])
            .unwrap();
        let out = op.apply(src);
        assert_eq!(op.apply(&out), out, "idempotent");
        out
    }

    #[test]
    fn a_match_arm_a_case_and_an_echo_list_are_patterns() {
        let src = "<?php\n$s = match ($x) {\n    Status::Draft => 'not yet',\n    default => 'done',\n};\n";
        assert_eq!(
            replace("Status::Draft => $V", "Status::Pending => $V", src),
            src.replace("Status::Draft", "Status::Pending")
        );
        let src = "<?php\nswitch ($n) {\n    case 0:\n        echo 'none';\n        break;\n}\n";
        assert_eq!(
            replace("case 0: $$$B", "case 1: $$$B", src),
            src.replace("case 0", "case 1")
        );
        let src = "<?php\necho json_encode($v), \"\\n\";\n";
        assert_eq!(
            replace("$A, \"\\n\"", "$A, PHP_EOL", src),
            "<?php\necho json_encode($v), PHP_EOL;\n"
        );
    }

    #[test]
    fn a_lone_sequence_body_matches_functions_and_methods_and_keeps_it() {
        // `{ $$$B }` parses only with the placeholder in an ERROR node; it
        // must still be read as a method too, not only as a function.
        let src = "<?php\nfunction q() { x(); }\nclass A {\n    function m() { y(); z(); }\n    function n($a) {}\n}\n";
        let op = crate::LANG
            .compile_pattern(
                "function $F() { $$$B }",
                PatternAction::Replace("function $F() { log(); $$$B }".into()),
                &[],
            )
            .unwrap();
        let found: Vec<String> = op
            .locate(src)
            .unwrap()
            .into_iter()
            .map(|f| f.text)
            .collect();
        assert_eq!(
            found,
            ["function q() { x(); }", "function m() { y(); z(); }"]
        );
        assert_eq!(
            op.apply(src),
            "<?php\nfunction q() { log(); x(); }\nclass A {\n    function m() { log(); y(); z(); }\n    function n($a) {}\n}\n"
        );
    }

    #[test]
    fn a_method_pattern_without_modifiers_matches_any_and_keeps_them() {
        let src = "<?php\nclass A {\n    function a() {}\n    public static function b() {}\n    #[Pure]\n    final protected function c() {}\n    function d() { return 1; }\n}\n";
        let any = crate::LANG
            .compile_pattern("function $F() {}", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(
            any.apply(src),
            "<?php\nclass A {\n    function d() { return 1; }\n}\n"
        );
        let fill = crate::LANG
            .compile_pattern(
                "function $F() {}",
                PatternAction::Replace("function $F() { run(); }".into()),
                &[],
            )
            .unwrap();
        assert_eq!(
            fill.apply(src),
            "<?php\nclass A {\n    function a() { run(); }\n    public static function b() { run(); }\n    #[Pure]\n    final protected function c() { run(); }\n    function d() { return 1; }\n}\n"
        );
        let exact = crate::LANG
            .compile_pattern("static function $F() {}", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(
            exact.apply("<?php\nclass A {\n    static function a() {}\n    public static function b() {}\n}\n"),
            "<?php\nclass A {\n    public static function b() {}\n}\n"
        );
    }

    #[test]
    fn a_deleted_function_takes_its_phpdoc() {
        let op = crate::LANG
            .compile_pattern("function $F() {}", PatternAction::Delete, &[])
            .unwrap();
        let src =
            "<?php\n// a note\n/** Old. */\nfunction old() {}\nfunction kept() { return 1; }\n";
        assert_eq!(
            op.apply(src),
            "<?php\n// a note\nfunction kept() { return 1; }\n"
        );
    }

    #[test]
    fn a_bare_call_pattern_also_finds_namespaced_calls() {
        let op = crate::LANG
            .compile_pattern("old($A)", PatternAction::Replace("zap($A)".into()), &[])
            .unwrap();
        let src = "<?php\nold(1);\nM\\old(2);\n\\M\\old(3);\n";
        assert_eq!(op.apply(src), "<?php\nzap(1);\nM\\zap(2);\n\\M\\zap(3);\n");
    }

    #[test]
    fn rewrites_call_with_two_captures() {
        let op = crate::LANG
            .compile_pattern(
                "$this->assertEquals($A, $B)",
                PatternAction::Replace("$this->assertSame($A, $B)".into()),
                &[],
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
        let op = crate::LANG
            .compile_pattern(
                "var_dump($$$ARGS)",
                PatternAction::Replace("dump($$$ARGS)".into()),
                &[],
            )
            .unwrap();
        let src = "<?php\nvar_dump($user, $order);\n";
        let out = op.apply(src);
        assert_eq!(out, "<?php\ndump($user, $order);\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = crate::LANG
            .compile_pattern("var_dump($X)", PatternAction::Delete, &[])
            .unwrap();
        let src = "<?php\nfunction f($x) {\n    var_dump($x);\n    return $x;\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "<?php\nfunction f($x) {\n    return $x;\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "<?php\nfunction f() {\n    $z = $foo->bar(1, 2);\n    $y = 3;\n}\n";
        for pattern in ["$foo->bar($A, $B)", "$y = $X;"] {
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
        let src = "<?php\nfunction f() {\n    $z = $legacy_name->bar(1, 2);\n    $y = 3;\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = crate::LANG
            .compile_pattern("function (", PatternAction::Delete, &[])
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("php::pattern"), "{err}");
    }

    #[test]
    fn matches_code_after_inline_html() {
        let op = crate::LANG
            .compile_pattern("var_dump($$$ARGS)", PatternAction::Delete, &[])
            .unwrap();
        let src =
            "<ul>\n<?php\nvar_dump($items);\nforeach ($items as $i) { echo $i; }\n?>\n</ul>\n";
        assert_eq!(
            op.apply(src),
            "<ul>\n<?php\nforeach ($items as $i) { echo $i; }\n?>\n</ul>\n"
        );
    }
}
