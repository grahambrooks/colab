//! C backend for colab.
//!
//! Three operation families:
//!
//! - `c::include` — rename, delete, ensure on `#include` directives.
//! - `c::symbol` — rename an identifier and its in-file usages.
//! - `c::call` — rewrite call expressions with a template.
//!
//! **`.h` is claimed by both this backend and `colab-lang-cpp`**, because
//! the extension alone cannot tell a C header from a C++ one. A script
//! mixing `c::` and `cpp::` rules will run both over `.h` files; each is
//! idempotent, so the result is the same either way, at the cost of one
//! extra parse per header.

#[cfg(test)]
mod calls;
pub mod includes;
mod patterns;
mod symbols;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about C ([`colab_rewrite::language`]).
pub static LANG: SourceLanguage = SourceLanguage {
    namespace: "c",
    extensions: &["c", "h"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &["comment", "string_content"],
    calls: CallShape::Fields("call_expression"),
    qualifiers: None,
    own: None,
};

/// `source` as colab's C tree (`colab-parse-c`, C dialect).
pub(crate) fn tree(source: &str) -> Option<colab_syntax::Tree> {
    colab_parse_c::parse_as(source, colab_parse_c::Dialect::C).ok()
}

/// Plug-in entry point for the `c` namespace.
pub struct CBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "include",
        description: "Rename, delete, or ensure a `#include` directive. Match strings are the bare path; both `<angle>` and \"quoted\" forms match, and rename preserves the existing style.",
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
        module: "symbol",
        description: "Rename a type, function, field, or variable and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
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
        "Match code by shape: the target is C code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `strcpy($DST, $SRC)`. An expression, statement, or declaration.",
    ),
];

impl LanguageBackend for CBackend {
    fn lang(&self) -> &'static str {
        "c"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("include")
    }

    fn fence_names(&self) -> Vec<&'static str> {
        vec!["c", "h"]
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
        "C source rewrites via colab's own C parser."
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
            (module, spec) => LANG.build_rule(self, module, spec),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn claims_c_sources_and_headers() {
        assert!(LANG.is_source(Path::new("a.c")));
        assert!(LANG.is_source(Path::new("a.h")));
        assert!(!LANG.is_source(Path::new("a.rs")));
        assert!(!LANG.is_source(Path::new("a")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = CBackend
            .build_rule("includ", RuleSpec::Delete { target: "x".into() })
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `c::includ`"), "{msg}");
        assert!(msg.contains("did you mean `include`?"), "{msg}");
    }

    #[test]
    fn unsupported_action_names_the_valid_ones() {
        let err = CBackend
            .build_rule("symbol", RuleSpec::Delete { target: "x".into() })
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("does not support the `delete` action"),
            "{msg}"
        );
        assert!(msg.contains("replace"), "{msg}");
    }
}
