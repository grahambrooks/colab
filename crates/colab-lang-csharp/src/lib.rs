//! C# backend for colab.
//!
//! Four operation families:
//!
//! - `csharp::using` — rename, delete, ensure on `using` directives
//!   (plain, `using static`, and the alias form).
//! - `csharp::namespace` — rename a `namespace` declaration, block or
//!   file-scoped.
//! - `csharp::symbol` — rename an identifier and its in-file usages.
//! - `csharp::call` — rewrite invocations with a template.

#[cfg(test)]
mod calls;
pub mod namespaces;
mod patterns;
mod symbols;
pub mod usings;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about C# ([`colab_rewrite::language`]). `.csx` is the C# scripting form.
pub(crate) static LANG: SourceLanguage = SourceLanguage {
    namespace: "csharp",
    extensions: &["cs", "csx"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &[
        "comment",
        "string_literal_content",
        "string_content",
        "verbatim_string_literal",
    ],
    calls: CallShape::Fields("invocation_expression"),
    qualifiers: None,
};

/// Plug-in entry point for the `csharp` namespace.
pub struct CSharpBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "using",
        description: "Rename, delete, or ensure a `using` directive. Covers the plain, `using static`, and alias forms; for an alias the target is the right-hand side.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched using name with another.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the matched `using` line.",
            },
            ActionCapability {
                name: "ensure",
                description: "Idempotently add `using <target>;` if missing.",
            },
        ],
    },
    Capability {
        module: "namespace",
        description: "Rewrite a `namespace` declaration, block-scoped or file-scoped.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Replace the matched namespace name with another.",
        }],
    },
    Capability {
        module: "symbol",
        description: "Rename a class, method, property, field, or local and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `identifier` whose text equals the target.",
        }],
    },
    Capability {
        module: "call",
        description: "Rewrite an invocation using a template with $1/$2/$args/$func placeholders.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace matched invocations with the rendered template. Rename the function to stay idempotent.",
        }],
    },
    colab_rewrite::language::pattern_capability(
        "Match code by shape: the target is C# code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `Assert.AreEqual($A, $B)`. An expression, statement, or declaration.",
    ),
];

impl LanguageBackend for CSharpBackend {
    fn lang(&self) -> &'static str {
        "csharp"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("using")
    }

    fn fence_names(&self) -> Vec<&'static str> {
        vec!["csharp", "cs", "c#"]
    }

    fn prose_ranges(&self, source: &str) -> Option<Vec<std::ops::Range<usize>>> {
        LANG.prose_ranges(source)
    }

    fn prose_locator(&self) -> Option<colab_core::ProseLocator> {
        Some(|source| LANG.prose_ranges(source))
    }

    fn build_rule_where(
        &self,
        module: &str,
        spec: RuleSpec,
        constraints: &[colab_core::Constraint],
    ) -> Result<Box<dyn Operation>> {
        LANG.build_rule_where(self, module, spec, constraints)
    }

    fn description(&self) -> &'static str {
        "C# source rewrites via tree-sitter-c-sharp."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn syntax_tree(&self, input: colab_core::AstInput<'_>) -> Result<colab_core::SyntaxTree> {
        LANG.syntax_tree(input)
    }

    fn generalize(&self, before: &str, after: &str) -> Result<Vec<colab_core::Generalized>> {
        LANG.generalize(before, after)
    }

    fn declarations(&self, source: &str) -> Option<Vec<colab_core::symbols::Declaration>> {
        Some(LANG.declarations(source))
    }

    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>> {
        match (module, spec) {
            (
                "using",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(usings::UsingRename {
                from: target,
                to: replacement,
            })),
            ("using", RuleSpec::Delete { target }) => Ok(Box::new(usings::UsingDelete { target })),
            ("using", RuleSpec::Ensure { target }) => Ok(Box::new(usings::UsingEnsure { target })),
            (
                "namespace",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(namespaces::NamespaceRename {
                from: target,
                to: replacement,
            })),
            (module, spec) => LANG.build_rule(self, module, spec),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn claims_cs_sources() {
        assert!(LANG.is_source(Path::new("a.cs")));
        assert!(LANG.is_source(Path::new("a.csx")));
        assert!(!LANG.is_source(Path::new("a.c")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = CSharpBackend
            .build_rule("usng", RuleSpec::Delete { target: "x".into() })
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `csharp::usng`"), "{msg}");
        assert!(msg.contains("did you mean `using`?"), "{msg}");
    }
}
