//! `LanguageBackend::generalize`: one example edit made into a rule that
//! then does the same change at other sites.

use colab_core::RuleSpec;

/// `before` → `after` generalized in `lang`, then applied to `elsewhere`.
fn learn_and_apply(
    lang: &str,
    before: &str,
    after: &str,
    elsewhere: &str,
) -> (colab_core::Generalized, String) {
    let registry = colab_backends::registry();
    let backend = registry.get(lang).unwrap();
    let rule = backend.generalize(before, after).unwrap();
    let op = backend
        .build_rule(
            "pattern",
            RuleSpec::Replace {
                target: rule.pattern.clone(),
                replacement: rule.template.clone(),
            },
        )
        .unwrap();
    let out = op.apply(elsewhere);
    (rule, out)
}

#[test]
fn a_dropped_argument_and_a_carried_one_become_metavariables() {
    let (rule, out) = learn_and_apply(
        "rust",
        "foo(config, Mode::Fast)",
        "foo(config)",
        "fn f() {\n    foo(self.cfg(), Mode::Slow);\n    bar(x, y);\n}\n",
    );
    assert_eq!(rule.pattern, "foo($A, $B)");
    assert_eq!(rule.template, "foo($A)");
    assert!(rule.warnings.is_empty(), "{:?}", rule.warnings);
    assert_eq!(out, "fn f() {\n    foo(self.cfg());\n    bar(x, y);\n}\n");
}

#[test]
fn a_callee_stays_literal_and_carried_values_move() {
    let (rule, out) = learn_and_apply(
        "go",
        "assert.Equal(t, want, got)",
        "require.Equal(t, got, want)",
        "package p\n\nfunc T() {\n\tassert.Equal(tt, 1, f(x))\n\tother.Equal(t, a, b)\n}\n",
    );
    assert_eq!(rule.pattern, "assert.Equal($A, $B, $C)");
    assert_eq!(rule.template, "require.Equal($A, $C, $B)");
    assert_eq!(
        out,
        "package p\n\nfunc T() {\n\trequire.Equal(tt, f(x), 1)\n\tother.Equal(t, a, b)\n}\n"
    );
}

#[test]
fn a_value_quoted_inside_a_longer_token_is_a_warning() {
    let registry = colab_backends::registry();
    let rule = registry
        .get("go")
        .unwrap()
        .generalize(
            r#"errors.Wrap(err, "read config")"#,
            r#"fmt.Errorf("read config: %w", err)"#,
        )
        .unwrap();
    assert_eq!(rule.pattern, "errors.Wrap($A, $B)");
    assert_eq!(rule.template, r#"fmt.Errorf("read config: %w", $A)"#);
    assert!(
        rule.warnings
            .iter()
            .any(|w| w.contains("`$B`") && w.contains("inside a longer token")),
        "{:?}",
        rule.warnings
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
    assert!(
        rust.generalize("fn (", "x")
            .unwrap_err()
            .to_string()
            .contains("does not parse")
    );
    // A config backend has no source grammar.
    assert!(registry.get("toml").unwrap().generalize("a", "b").is_err());
}
