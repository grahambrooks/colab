//! Contract tests that run against *every* registered backend.
//!
//! These exist because **`Operation::prefilter` must be a necessary
//! condition, or `None`** — a rule previously enforced only by
//! documentation and reviewer attention, across 44 modules. The walker
//! skips the tree-sitter parse entirely when the literal is absent, so a
//! prefilter that is not implied by the match is a *silent* correctness
//! bug: the rule stops firing on exactly the files that need it.
//! `ensure`-style operations must return `None`, since they act when the
//! target is missing.
//!
//! Being generic over the registry is the point: a new backend inherits
//! these checks without anyone remembering to add them.
//!
//! The companion DSL-spellability checks live in
//! `colab-dsl/tests/namespace_spelling.rs` — they need the grammar, and
//! putting them here would make colab-backends and colab-dsl dev-depend
//! on each other.

use colab_core::{BackendRegistry, RuleSpec};

/// A target string no real source file will contain, used to build a rule
/// whose prefilter can be reasoned about without knowing the language.
const SENTINEL: &str = "Zq_colab_absent_target_9174";

/// Source samples that must not contain [`SENTINEL`]. One per language
/// family; the specific language does not matter because a prefiltered
/// operation must be a no-op on *any* source lacking its literal.
const SAMPLES: &[&str] = &[
    "",
    "\n",
    "package main\n\nimport \"fmt\"\n\nfunc main() { fmt.Println(1) }\n",
    "class A { void m() { helper(1); } }\n",
    "#include <stdio.h>\nint main(void) { return 0; }\n",
    "<?php\nnamespace App;\nuse App\\Thing;\nThing::run($a);\n",
    "require 'json'\nclass Service\n  def call; end\nend\n",
    "import Foundation\nfunc f() { g(1) }\n",
    "use std::fmt;\nfn main() { println!(\"x\"); }\n",
    "def f():\n    return 1\n",
    "const x = require('y');\nexport default x;\n",
    "[dependencies]\nserde = \"1\"\n",
];

/// Build one rule per (module, action) the registry advertises, using
/// [`SENTINEL`] as the target so every operation is guaranteed to have
/// nothing to match.
fn sentinel_rules(
    registry: &BackendRegistry,
) -> Vec<(String, String, &'static str, Box<dyn colab_core::Operation>)> {
    let mut out = Vec::new();
    for lang in registry.languages() {
        let backend = registry.get(lang).expect("registry lists it");
        for capability in backend.capabilities() {
            for action in capability.actions {
                let spec = match action.name {
                    "replace" => RuleSpec::Replace {
                        target: SENTINEL.to_string(),
                        replacement: "Zq_colab_replacement".to_string(),
                    },
                    "delete" => RuleSpec::Delete {
                        target: SENTINEL.to_string(),
                    },
                    "ensure" => RuleSpec::Ensure {
                        target: SENTINEL.to_string(),
                    },
                    "replace_call" => RuleSpec::ReplaceCall {
                        target: SENTINEL.to_string(),
                        template: "Zq_colab_replacement($args)".to_string(),
                    },
                    other => panic!("unhandled action `{other}` — add it to this test"),
                };
                let op = backend
                    .build_rule(capability.module, spec)
                    .unwrap_or_else(|e| {
                        panic!("{lang}::{} advertises `{}` but build_rule rejected it: {e}",
                               capability.module, action.name)
                    });
                out.push((
                    lang.to_string(),
                    capability.module.to_string(),
                    action.name,
                    op,
                ));
            }
        }
    }
    out
}

#[test]
fn ensure_operations_have_no_prefilter() {
    let registry = colab_backends::registry();
    let mut checked = 0;
    for (lang, module, action, op) in sentinel_rules(&registry) {
        if action != "ensure" {
            continue;
        }
        assert_eq!(
            op.prefilter(),
            None,
            "{lang}::{module} `ensure` returns a prefilter. `ensure` acts when its \
             target is ABSENT, so a prefilter suppresses it in exactly the files \
             that need the insert."
        );
        checked += 1;
    }
    assert!(checked > 0, "no ensure operations found — test is vacuous");
}

#[test]
fn a_prefilter_is_derived_from_the_match_target() {
    // A prefilter must be implied by the target. Requiring it to be a
    // substring of the target is a strong, language-agnostic proxy: it
    // rules out a literal drawn from somewhere else, which could be
    // absent from a file the rule should still change.
    let registry = colab_backends::registry();
    let mut checked = 0;
    for (lang, module, action, op) in sentinel_rules(&registry) {
        if let Some(literal) = op.prefilter() {
            assert!(
                !literal.is_empty(),
                "{lang}::{module} `{action}` returns an empty prefilter, which \
                 filters nothing — return None instead"
            );
            assert!(
                SENTINEL.contains(literal),
                "{lang}::{module} `{action}` prefilter {literal:?} is not derived \
                 from the target {SENTINEL:?}; a literal from elsewhere may be \
                 absent from files the rule should still change"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no prefiltered operations found — test is vacuous");
}

#[test]
fn absence_of_the_prefilter_literal_implies_a_no_op() {
    // The contract itself: if the literal is not in the source, applying
    // the operation must change nothing. The walker relies on this to
    // skip the parse.
    let registry = colab_backends::registry();
    for (lang, module, action, op) in sentinel_rules(&registry) {
        let Some(literal) = op.prefilter() else {
            continue;
        };
        for sample in SAMPLES {
            assert!(
                !sample.contains(literal),
                "sample unexpectedly contains {literal:?} — fix the test fixture"
            );
            assert_eq!(
                op.apply(sample),
                *sample,
                "{lang}::{module} `{action}` changed a source that does not contain \
                 its prefilter literal {literal:?}. The walker skips the parse in \
                 this case, so the rule would silently not fire."
            );
        }
    }
}

#[test]
fn every_advertised_action_actually_builds() {
    // `capabilities()` drives `colab schema`, `list-rules`, and LSP
    // completion. Advertising an action that `build_rule` rejects would
    // send an agent down a dead end. `sentinel_rules` panics on mismatch,
    // so simply constructing them is the assertion.
    let registry = colab_backends::registry();
    let rules = sentinel_rules(&registry);
    assert!(
        rules.len() >= 40,
        "expected the full capability matrix, built only {}",
        rules.len()
    );
}

#[test]
fn no_backend_claims_a_path_it_cannot_parse() {
    // Every operation must reject paths outside its language. A backend
    // whose `is_file_relevant` accepted everything would parse the whole
    // repo with the wrong grammar.
    let registry = colab_backends::registry();
    for (lang, module, action, op) in sentinel_rules(&registry) {
        assert!(
            !op.is_file_relevant(std::path::Path::new("a.definitely_not_a_language")),
            "{lang}::{module} `{action}` claims an unknown extension"
        );
    }
}
