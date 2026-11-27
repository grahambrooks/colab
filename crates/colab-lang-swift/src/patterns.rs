//! `swift::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Swift's grammar, expando character, and the contexts that
//! let a fragment that is not a valid Swift *file* on its own parse.

use colab_rewrite::pattern::PatternLanguage;

/// `$0` is a Swift closure argument, so metavariables are parsed as `µ`.
/// Contexts, in order: a statement or expression inside a function body,
/// then a class member, then a protocol's requirement (`func $F() async`).
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        syntax: colab_rewrite::pattern::Syntax::Own(&colab_parse_swift::PARSER),
        expando: 'µ',
        contexts: &[
            "func f() {\n@@PATTERN@@\n}\n",
            "class C {\n@@PATTERN@@\n}\n",
            "protocol _P {\n@@PATTERN@@\n}\n",
        ],
        also: &[],
        qualified_calls: None,
        // Doc comments document the declaration after them.
        leading: colab_rewrite::pattern::Leading {
            kinds: &[],
            doc_comments: &["///", "/**"],
        },
        macro_contents: None,
        // One `modifiers` node holds the keywords and attributes alike.
        modifiers: Some(colab_rewrite::pattern::Modifiers {
            kinds: &["modifiers"],
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

    #[test]
    fn a_protocol_requirement_is_a_pattern() {
        let op = crate::LANG
            .compile_pattern(
                "func readings(for $S: String) async throws -> [Reading]",
                PatternAction::Replace(
                    "func readings(at $S: String) async throws -> [Reading]".into(),
                ),
                &[],
            )
            .unwrap();
        let src = "protocol Source {\n    func readings(for station: String) async throws -> [Reading]\n}\n";
        let out = op.apply(src);
        assert_eq!(out, src.replace("(for ", "(at "));
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn a_function_pattern_without_modifiers_matches_any_and_keeps_them() {
        let src = "class A {\n    func a() {}\n    public static func b() {}\n    @objc private func c() {}\n    func d() -> Int { return 1 }\n}\n";
        let any = crate::LANG
            .compile_pattern("func $F() {}", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(
            any.apply(src),
            "class A {\n    func d() -> Int { return 1 }\n}\n"
        );
        let fill = crate::LANG
            .compile_pattern(
                "func $F() {}",
                PatternAction::Replace("func $F() { run() }".into()),
                &[],
            )
            .unwrap();
        assert_eq!(
            fill.apply(src),
            "class A {\n    func a() { run() }\n    public static func b() { run() }\n    @objc private func c() { run() }\n    func d() -> Int { return 1 }\n}\n"
        );
        let exact = crate::LANG
            .compile_pattern("public func $F() {}", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(
            exact.apply("class A {\n    func a() {}\n    public func b() {}\n    public static func c() {}\n}\n"),
            "class A {\n    func a() {}\n    public static func c() {}\n}\n"
        );
    }

    #[test]
    fn a_deleted_function_takes_its_doc_comment() {
        let op = crate::LANG
            .compile_pattern("func $F() {}", PatternAction::Delete, &[])
            .unwrap();
        let src = "// a note\n/// Old.\nfunc old() {}\nfunc kept() -> Int { return 1 }\n";
        assert_eq!(
            op.apply(src),
            "// a note\nfunc kept() -> Int { return 1 }\n"
        );
    }

    #[test]
    fn rewrites_call_with_two_captures() {
        let op = crate::LANG
            .compile_pattern(
                "XCTAssertEqual($A, $B)",
                PatternAction::Replace("XCTAssertTrue($A == $B)".into()),
                &[],
            )
            .unwrap();
        let src = "func testAdd() {\n    XCTAssertEqual(add(2, 2), 4)\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "func testAdd() {\n    XCTAssertTrue(add(2, 2) == 4)\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "swift::pattern 'XCTAssertEqual($A, $B)' -> 'XCTAssertTrue($A == $B)'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = crate::LANG
            .compile_pattern(
                "NSLog($$$ARGS)",
                PatternAction::Replace("logger.info($$$ARGS)".into()),
                &[],
            )
            .unwrap();
        let src = "func run(n: Int) {\n    NSLog(\"n=%d\", n)\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "func run(n: Int) {\n    logger.info(\"n=%d\", n)\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = crate::LANG
            .compile_pattern("print($$$ARGS)", PatternAction::Delete, &[])
            .unwrap();
        let src = "func run(n: Int) {\n    print(\"n\", n)\n    use(n)\n}\n";
        let out = op.apply(src);
        assert_eq!(out, "func run(n: Int) {\n    use(n)\n}\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "func f() -> Int {\n    let y = foo.bar(1, 2)\n    return y\n}\n";
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
        let src = "func f() -> Int {\n    let y = legacy_name.bar(1, 2)\n    return y\n}\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = crate::LANG
            .compile_pattern("func (", PatternAction::Delete, &[])
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("swift::pattern"), "{err}");
    }
}
