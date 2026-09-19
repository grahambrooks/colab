//! Every registered module name must be spellable as a DSL namespace.
//!
//! Module names are chosen to match each language's own construct, and
//! C's is `#include` — which collided with the `include` pack directive
//! and made `c::include` unparseable. That was found via a confusing
//! corpus parse error rather than a targeted test.
//!
//! These live here rather than in `colab-backends` because they need the
//! grammar: colab-dsl already dev-depends on colab-backends, so testing
//! from this side avoids a dev-dependency cycle between the two.

#[test]
fn every_module_name_is_spellable_in_the_dsl() {
    // Guards the `c::include` class of bug: a module named after a
    // language construct that happens to be a DSL keyword.
    let registry = colab_backends::registry();
    let mut checked = 0;
    for lang in registry.languages() {
        let backend = registry.get(lang).expect("registry lists it");
        for capability in backend.capabilities() {
            let action = capability
                .actions
                .first()
                .expect("a module with no actions is unusable");
            let body = match action.name {
                "replace" => "replace \"y\"".to_string(),
                "delete" => "delete".to_string(),
                "ensure" => "ensure".to_string(),
                "replace_call" => "replace_call \"y($args)\"".to_string(),
                "set" => "set '1'".to_string(),
                "insert" => "insert '1'".to_string(),
                other => panic!("unhandled action `{other}`"),
            };
            let script = format!(
                "refactor \"t\" {{ match {lang}::{} \"x\" {{ {body} }} }}",
                capability.module
            );
            colab_dsl::parse(&script).unwrap_or_else(|e| {
                panic!(
                    "`{lang}::{}` does not parse as a namespace — is `{}` a DSL \
                     keyword? Add it to the Identifier rule in codemod.lalrpop. \
                     Error: {e}",
                    capability.module, capability.module
                )
            });
            checked += 1;
        }
    }
    assert!(
        checked >= 12,
        "expected at least one module per backend, saw {checked}"
    );
}

#[test]
fn the_keyword_guard_can_actually_fail() {
    // A guard that cannot fail is worthless. `refactor` is a DSL keyword
    // that is deliberately *not* in the grammar's Identifier escape list,
    // so a module named `refactor` would not parse — demonstrating that
    // `every_module_name_is_spellable_in_the_dsl` detects a real
    // collision rather than passing vacuously.
    //
    // If this ever starts passing, the escape list grew to cover
    // `refactor` and this test needs a different un-escaped keyword.
    let script = "refactor \"t\" { match c::refactor \"x\" { delete } }";
    assert!(
        colab_dsl::parse(script).is_err(),
        "`refactor` is now spellable as a module name — pick another \
         un-escaped keyword so this guard stays meaningful"
    );

    // And the positive control: an escaped keyword does parse.
    let ok = "refactor \"t\" { match c::include \"x\" { delete } }";
    assert!(
        colab_dsl::parse(ok).is_ok(),
        "`c::include` must parse — the grammar escapes `include` for exactly this"
    );
}

#[test]
fn every_module_name_is_also_scopable() {
    // The `in "<glob>"` clause sits between the target and the action, so
    // a keyword collision could in principle break only the scoped form.
    let registry = colab_backends::registry();
    for lang in registry.languages() {
        let backend = registry.get(lang).expect("registry lists it");
        for capability in backend.capabilities() {
            let action = capability.actions.first().expect("at least one action");
            let body = match action.name {
                "replace" => "replace \"y\"",
                "delete" => "delete",
                "ensure" => "ensure",
                "replace_call" => "replace_call \"y($args)\"",
                other => panic!("unhandled action `{other}`"),
            };
            let script = format!(
                "refactor \"t\" {{ match {lang}::{} \"x\" in \"src/**\" {{ {body} }} }}",
                capability.module
            );
            colab_dsl::parse(&script).unwrap_or_else(|e| {
                panic!("scoped `{lang}::{}` fails to parse: {e}", capability.module)
            });
        }
    }
}
