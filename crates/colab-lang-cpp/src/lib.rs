//! C++ backend for colab.
//!
//! Four operation families:
//!
//! - `cpp::include` — rename, delete, ensure on `#include` directives.
//! - `cpp::namespace` — rename a `namespace` declaration.
//! - `cpp::symbol` — rename an identifier and its in-file usages.
//! - `cpp::call` — rewrite call expressions with a template.
//!
//! **`.h` is claimed by both this backend and `colab-lang-c`**, because
//! the extension alone cannot tell a C header from a C++ one. A script
//! mixing `c::` and `cpp::` rules runs both over `.h` files; each is
//! idempotent, so the result is the same either way, at the cost of one
//! extra parse per header.

#[cfg(test)]
mod calls;
pub mod includes;
pub mod namespaces;
mod patterns;
mod symbols;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about C++ ([`colab_rewrite::language`]). `.h` overlaps with
/// `colab-lang-c` deliberately — see the module docs.
pub static LANG: SourceLanguage = SourceLanguage {
    namespace: "cpp",
    extensions: &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "h++", "h"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &["comment", "string_content", "raw_string_content"],
    calls: CallShape::Fields("call_expression"),
    qualifiers: None,
    own: None,
};

/// `source` as colab's C++ tree (`colab-parse-c`).
pub(crate) fn tree(source: &str) -> Option<colab_syntax::Tree> {
    colab_parse_c::parse(source).ok()
}

/// Plug-in entry point for the `cpp` namespace.
pub struct CppBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "include",
        description: "Rename, delete, or ensure a `#include` directive. Match strings are the bare path; both <angle> and \"quoted\" forms match, and rename preserves the existing style.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched include path with another.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the matched `#include` line.",
            },
            ActionCapability {
                name: "ensure",
                description: "Idempotently add the include if missing. Wrap the target in <> to insert the system form.",
            },
        ],
    },
    Capability {
        module: "namespace",
        description: "Rename a `namespace` declaration. Matches the declared name exactly, including the nested `a::b` form.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Replace the matched namespace name with another.",
        }],
    },
    Capability {
        module: "symbol",
        description: "Rename a class, function, field, or variable and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every identifier-kind node whose text equals the target.",
        }],
    },
    Capability {
        module: "call",
        description: "Rewrite a call expression using a template with $1/$2/$args/$func placeholders.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace matched calls with the rendered template. Rename the function to stay idempotent.",
        }],
    },
    colab_rewrite::language::pattern_capability(
        "Match code by shape: the target is C++ code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `std::make_pair($A, $B)`. An expression, statement, or declaration.",
    ),
];

impl LanguageBackend for CppBackend {
    fn lang(&self) -> &'static str {
        "cpp"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("include")
    }

    fn fence_names(&self) -> Vec<&'static str> {
        vec!["cpp", "c++", "cc", "cxx", "hpp"]
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
        "C++ source rewrites via colab's own C++ parser."
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
                "include",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(includes::IncludeRename {
                from: target,
                to: replacement,
            })),
            ("include", RuleSpec::Delete { target }) => {
                Ok(Box::new(includes::IncludeDelete { target }))
            }
            ("include", RuleSpec::Ensure { target }) => {
                Ok(Box::new(includes::IncludeEnsure { target }))
            }
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
    fn claims_cpp_sources_and_headers() {
        for ext in ["cpp", "cc", "cxx", "hpp", "hh", "h"] {
            assert!(LANG.is_source(Path::new(&format!("a.{ext}"))), "{ext}");
        }
        assert!(!LANG.is_source(Path::new("a.rs")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = CppBackend
            .build_rule(
                "namespce",
                RuleSpec::Replace {
                    target: "a".into(),
                    replacement: "b".into(),
                },
            )
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `cpp::namespce`"), "{msg}");
        assert!(msg.contains("did you mean `namespace`?"), "{msg}");
    }
}
