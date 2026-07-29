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

pub mod calls;
pub mod includes;
pub mod symbols;

use std::cell::RefCell;
use std::path::Path;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use tree_sitter::{Parser, Tree};

/// Extensions this backend claims.
const RELEVANT_EXTENSIONS: &[&str] = &["c", "h"];

pub(crate) fn is_relevant(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|ext| RELEVANT_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

thread_local! {
    /// Per-thread tree-sitter C parser. Reused across files so
    /// `Parser::new() + set_language()` only happens once per rayon
    /// worker.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_c::LANGUAGE.into())
            .expect("failed to load tree-sitter C grammar");
        p
    });
}

/// Parse `source` into a C syntax tree using the thread-local parser.
/// Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
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
];

impl LanguageBackend for CBackend {
    fn lang(&self) -> &'static str {
        "c"
    }

    fn description(&self) -> &'static str {
        "C source rewrites via tree-sitter-c."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
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
                "symbol",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(symbols::SymbolRename {
                from: target,
                to: replacement,
            })),
            ("call", RuleSpec::ReplaceCall { target, template }) => {
                Ok(Box::new(calls::CallReplace {
                    function: target,
                    template,
                }))
            }
            (other, spec) => Err(self.unsupported(other, &spec)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_c_sources_and_headers() {
        assert!(is_relevant(Path::new("a.c")));
        assert!(is_relevant(Path::new("a.h")));
        assert!(!is_relevant(Path::new("a.rs")));
        assert!(!is_relevant(Path::new("a")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = CBackend
            .build_rule(
                "includ",
                RuleSpec::Delete {
                    target: "x".into(),
                },
            )
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `c::includ`"), "{msg}");
        assert!(msg.contains("did you mean `include`?"), "{msg}");
    }

    #[test]
    fn unsupported_action_names_the_valid_ones() {
        let err = CBackend
            .build_rule(
                "symbol",
                RuleSpec::Delete {
                    target: "x".into(),
                },
            )
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("does not support the `delete` action"), "{msg}");
        assert!(msg.contains("replace"), "{msg}");
    }
}
