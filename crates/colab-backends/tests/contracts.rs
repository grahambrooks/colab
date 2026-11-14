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
                // A JSON Pointer must start with `/`; the sentinel is still a
                // substring, so the prefilter checks below are unaffected.
                let target = if lang == "json" {
                    format!("/{SENTINEL}")
                } else {
                    SENTINEL.to_string()
                };
                let spec = match action.name {
                    "replace" => RuleSpec::Replace {
                        target: target.clone(),
                        replacement: "Zq_colab_replacement".to_string(),
                    },
                    "delete" => RuleSpec::Delete {
                        target: target.clone(),
                    },
                    "ensure" => RuleSpec::Ensure {
                        target: target.clone(),
                    },
                    "replace_call" => RuleSpec::ReplaceCall {
                        target: target.clone(),
                        template: "Zq_colab_replacement($args)".to_string(),
                    },
                    // `1` is a valid value in every config syntax.
                    "set" => RuleSpec::Set {
                        target: target.clone(),
                        value: "1".to_string(),
                    },
                    "insert" => RuleSpec::Insert {
                        target: target.clone(),
                        value: "1".to_string(),
                    },
                    other => panic!("unhandled action `{other}` — add it to this test"),
                };
                let op = backend
                    .build_rule(capability.module, spec)
                    .unwrap_or_else(|e| {
                        panic!(
                            "{lang}::{} advertises `{}` but build_rule rejected it: {e}",
                            capability.module, action.name
                        )
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
        // `set` and `insert` also act when their target is absent.
        if !matches!(action, "ensure" | "set" | "insert") {
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
    assert!(
        checked > 0,
        "no prefiltered operations found — test is vacuous"
    );
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

// ---------- M26: `where` constraints and import clauses -------------------

fn unbound_constraint() -> Vec<colab_core::Constraint> {
    vec![colab_core::Constraint {
        negated: false,
        test: colab_core::Test::Matches {
            var: colab_core::Var::parse("$ZZ").expect("valid metavariable"),
            regex: "x".into(),
        },
    }]
}

#[test]
fn only_pattern_modules_take_a_where_clause() {
    let registry = colab_backends::registry();
    for lang in registry.languages() {
        let backend = registry.get(lang).unwrap();
        for cap in backend.capabilities() {
            let spec = RuleSpec::Delete {
                target: "$A".into(),
            };
            let err = backend
                .build_rule_where(cap.module, spec, &unbound_constraint())
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            let rejected_as_unsupported = err.contains("does not take a `where` clause");
            if cap.module == "pattern" {
                // Reaching the constraint check (or the pattern compiler)
                // means the backend forwards constraints to the engine.
                assert!(
                    !rejected_as_unsupported && !err.is_empty(),
                    "`{lang}::pattern` must compile `where` constraints through \
                     colab_rewrite::pattern::build_rule_where; got: {err:?}"
                );
            } else {
                assert!(
                    rejected_as_unsupported,
                    "`{lang}::{}` should reject `where`; got: {err:?}",
                    cap.module
                );
            }
        }
    }
}

#[test]
fn the_import_module_supports_ensure() {
    let registry = colab_backends::registry();
    for lang in registry.languages() {
        let backend = registry.get(lang).unwrap();
        let has_pattern = backend.capabilities().iter().any(|c| c.module == "pattern");
        let Some(module) = backend.import_module() else {
            assert!(
                !has_pattern,
                "`{lang}` has `pattern` but no import_module, so `requires import` fails"
            );
            continue;
        };
        let cap = backend
            .capabilities()
            .iter()
            .find(|c| c.module == module)
            .unwrap_or_else(|| panic!("`{lang}`'s import_module `{module}` is not a module"));
        assert!(
            cap.actions.iter().any(|a| a.name == "ensure"),
            "`{lang}::{module}` must support `ensure` for `requires import`"
        );
    }
}

/// Every source backend finds prose — comments and string contents — and
/// only prose: `Needle` appears once in a comment, once in a string, and
/// once as code, so exactly two occurrences fall inside its ranges.
#[test]
fn prose_ranges_cover_comments_and_strings_but_not_code() {
    let samples: &[(&str, &str)] = &[
        ("c", "// Needle\nconst char *s = \"Needle\";\nint Needle;\n"),
        ("cpp", "// Needle\nauto s = \"Needle\";\nint Needle;\n"),
        (
            "csharp",
            "// Needle\nclass A { string s = \"Needle\"; int Needle; }\n",
        ),
        (
            "go",
            "package p\n// Needle\nvar s = \"Needle\"\nvar Needle int\n",
        ),
        (
            "java",
            "class A { // Needle\n String s = \"Needle\"; int Needle; }\n",
        ),
        ("js", "// Needle\nconst s = \"Needle\";\nlet Needle;\n"),
        ("kotlin", "// Needle\nval s = \"Needle\"\nval Needle = 1\n"),
        (
            "php",
            "<?php\n// Needle\n$s = \"Needle\";\nfunction Needle() {}\n",
        ),
        ("python", "# Needle\ns = \"Needle\"\nNeedle = 1\n"),
        ("ruby", "# Needle\ns = \"Needle\"\nNeedle = 1\n"),
        (
            "rust",
            "// Needle\nconst S: &str = \"Needle\";\nfn Needle() {}\n",
        ),
        ("swift", "// Needle\nlet s = \"Needle\"\nlet Needle = 1\n"),
    ];
    let registry = colab_backends::registry();
    for (lang, source) in samples {
        let ranges = registry
            .get(lang)
            .unwrap()
            .prose_ranges(source)
            .unwrap_or_else(|| panic!("{lang} has no prose_ranges"));
        let inside = source
            .match_indices("Needle")
            .filter(|(at, _)| ranges.iter().any(|r| r.start <= *at && at + 6 <= r.end))
            .count();
        assert_eq!(inside, 2, "{lang}: {ranges:?}");
    }
    for lang in ["json", "markdown", "toml"] {
        assert!(
            registry.get(lang).unwrap().prose_ranges("x").is_none(),
            "{lang}"
        );
    }
}
