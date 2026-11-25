//! `LanguageBackend::generalize`: one example edit made into rules that
//! then do the same change at other sites.

use colab_core::RuleSpec;

/// `before` → `after` generalized in `lang`: the rules, as (pattern,
/// template) pairs.
fn learn(lang: &str, before: &str, after: &str) -> Vec<colab_core::Generalized> {
    colab_backends::registry()
        .get(lang)
        .unwrap()
        .generalize(before, after)
        .unwrap()
}

/// Every rule `learn` gives, applied in turn to `elsewhere`.
fn apply(lang: &str, rules: &[colab_core::Generalized], elsewhere: &str) -> String {
    let registry = colab_backends::registry();
    let backend = registry.get(lang).unwrap();
    rules.iter().fold(elsewhere.to_string(), |text, rule| {
        backend
            .build_rule(
                "pattern",
                RuleSpec::Replace {
                    target: rule.pattern.clone(),
                    replacement: rule.template.clone(),
                },
            )
            .unwrap()
            .apply(&text)
    })
}

fn pairs(rules: &[colab_core::Generalized]) -> Vec<(&str, &str)> {
    rules
        .iter()
        .map(|r| (r.pattern.as_str(), r.template.as_str()))
        .collect()
}

/// From the 2026-11-24 replay: a `"."` inside a macro's token tree was
/// bound as `$A`, and the template's `args.source()` became `$B$Asource()`.
#[test]
fn punctuation_and_string_contents_are_never_bound() {
    let rules = learn(
        "rust",
        "let source = make(args, vec![PathBuf::from(\".\")]);",
        "let source = args.source();",
    );
    for rule in &rules {
        assert!(!rule.pattern.contains("\"$"), "{:?}", pairs(&rules));
        assert!(
            !rule.template.contains("$A$") && !rule.template.contains("$B$"),
            "{:?}",
            pairs(&rules)
        );
        assert!(rule.template.contains(".source()"), "{:?}", pairs(&rules));
    }
}

#[test]
fn a_dropped_argument_and_a_carried_one_become_metavariables() {
    let rules = learn("rust", "foo(config, Mode::Fast)", "foo(config)");
    assert_eq!(pairs(&rules), [("foo($A, $B)", "foo($A)")]);
    assert!(rules[0].warnings.is_empty(), "{:?}", rules[0].warnings);
    assert_eq!(
        apply(
            "rust",
            &rules,
            "fn f() {\n    foo(self.cfg(), Mode::Slow);\n    bar(x, y);\n}\n"
        ),
        "fn f() {\n    foo(self.cfg());\n    bar(x, y);\n}\n"
    );
}

#[test]
fn a_callee_stays_literal_and_carried_values_move() {
    let rules = learn(
        "go",
        "assert.Equal(t, want, got)",
        "require.Equal(t, got, want)",
    );
    assert_eq!(
        pairs(&rules),
        [("assert.Equal($A, $B, $C)", "require.Equal($A, $C, $B)")]
    );
    assert_eq!(
        apply(
            "go",
            &rules,
            "package p\n\nfunc T() {\n\tassert.Equal(tt, 1, f(x))\n\tother.Equal(t, a, b)\n}\n"
        ),
        "package p\n\nfunc T() {\n\trequire.Equal(tt, f(x), 1)\n\tother.Equal(t, a, b)\n}\n"
    );
}

#[test]
fn a_changed_callee_alone_lifts_to_its_call() {
    // Only `assert` → `require` differs: the rule is over the call, its
    // arguments free.
    let rules = learn("go", "assert.Equal(t, 1, got)", "require.Equal(t, 1, got)");
    assert_eq!(
        pairs(&rules),
        [("assert.Equal($A, $B, $C)", "require.Equal($A, $B, $C)")]
    );
}

#[test]
fn context_lines_around_the_change_are_left_out() {
    // An Edit's `old_string` carries unchanged lines for uniqueness.
    let before = "let a = load(&path)?;\nlet b = old_call(a, 1);\nprintln!(\"{b}\");";
    let after = "let a = load(&path)?;\nlet b = new_call(a);\nprintln!(\"{b}\");";
    let rules = learn("rust", before, after);
    assert_eq!(pairs(&rules), [("old_call($A, $B)", "new_call($A)")]);
}

#[test]
fn two_places_changed_give_a_rule_each() {
    let before = "fn f() {\n    first(a, 1);\n    keep();\n    second(b);\n}";
    let after = "fn f() {\n    first(a);\n    keep();\n    second2(b);\n}";
    let rules = learn("rust", before, after);
    assert_eq!(
        pairs(&rules),
        [
            ("first($A, $B)", "first($A)"),
            ("second($A)", "second2($A)")
        ]
    );
}

#[test]
fn a_value_quoted_inside_a_longer_token_is_a_warning() {
    let rules = learn(
        "go",
        r#"errors.Wrap(err, "read config")"#,
        r#"fmt.Errorf("read config: %w", err)"#,
    );
    assert_eq!(
        pairs(&rules),
        [(
            "errors.Wrap($A, $B)",
            r#"fmt.Errorf("read config: %w", $A)"#
        )]
    );
    assert!(
        rules[0]
            .warnings
            .iter()
            .any(|w| w.contains("`$B`") && w.contains("inside a longer token")),
        "{:?}",
        rules[0].warnings
    );
}

#[test]
fn a_changed_string_argument_lifts_to_the_call_not_its_argument_list() {
    // The string alone has nothing to vary; its argument list, alone, is a
    // parenthesized sequence in JavaScript. The rule is the call.
    let rules = learn(
        "js",
        r#"fs.writeFile(path, data, "utf8")"#,
        r#"fs.writeFile(path, data, "latin1")"#,
    );
    assert_eq!(
        pairs(&rules),
        [(
            "fs.writeFile($A, $B, $C)",
            r#"fs.writeFile($A, $B, "latin1")"#
        )]
    );
    assert_eq!(
        apply("js", &rules, "fs.writeFile(out, text, \"utf8\");\n"),
        "fs.writeFile(out, text, \"latin1\");\n"
    );
}

#[test]
fn a_string_quote_is_not_a_value() {
    // Python names a string's quotes (`string_start`, `string_end`); the
    // quote `"` is in `after` too, but it is no value to carry over.
    let rules = learn(
        "python",
        r#"Library("Main Street")"#,
        r#"Library("High Street")"#,
    );
    assert_eq!(
        pairs(&rules),
        [("Library($A)", r#"Library("High Street")"#)]
    );
}

#[test]
fn a_callee_in_no_field_stays_literal() {
    // Kotlin's and Swift's calls give the callee no `function` field.
    for (lang, before, after, rule) in [
        (
            "kotlin",
            "Random(42)",
            "Random(7)",
            ("Random($A)", "Random(7)"),
        ),
        (
            "kotlin",
            "playlist.best(1)",
            "playlist.best(3)",
            ("playlist.best($A)", "playlist.best(3)"),
        ),
        (
            "swift",
            "hottest(1)",
            "hottest(3)",
            ("hottest($A)", "hottest(3)"),
        ),
    ] {
        assert_eq!(
            pairs(&learn(lang, before, after)),
            [rule],
            "{lang}: {before}"
        );
    }
    let rules = learn("kotlin", "Random(42)", "Random(7)");
    assert_eq!(
        apply("kotlin", &rules, "fun f() {\n    g(Random(1), h(2))\n}\n"),
        "fun f() {\n    g(Random(7), h(2))\n}\n"
    );
}

#[test]
fn a_swift_call_suffix_stands_for_its_call() {
    // Swift wraps a call's arguments in `call_suffix`; alone, the suffix
    // `("airport", 12.0)` is a tuple, and the rule lost its callee.
    let rules = learn(
        "swift",
        r#"makeReading("airport", 12.0)"#,
        r#"makeReading("harbour", 12.0)"#,
    );
    assert_eq!(
        pairs(&rules),
        [("makeReading($A, $B)", r#"makeReading("harbour", $B)"#)]
    );
}

#[test]
fn changed_lines_that_are_no_code_alone_keep_their_context() {
    // Only the middle line changed, and `"Sunday",` alone does not parse:
    // the whole call is the snippet.
    let before = "Playlist.of(\n    \"Sunday\",\n    track,\n)";
    let after = "Playlist.of(\n    \"Monday\",\n    track,\n)";
    let rules = learn("kotlin", before, after);
    assert_eq!(rules.len(), 1, "{rules:?}");
    assert_eq!(
        apply("kotlin", &rules, &format!("fun f() {{\n    {before}\n}}\n")),
        format!("fun f() {{\n    {after}\n}}\n")
    );
}

#[test]
fn mistakes_are_explained() {
    let registry = colab_backends::registry();
    let rust = registry.get("rust").unwrap();
    assert!(
        rust.generalize("f(x)", "f(x)")
            .unwrap_err()
            .to_string()
            .contains("the same")
    );
    assert!(rust.generalize("fn (", "x").is_err());
    // A config backend has no source grammar.
    assert!(registry.get("toml").unwrap().generalize("a", "b").is_err());
}
