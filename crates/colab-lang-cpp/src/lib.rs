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

pub mod calls;
pub mod includes;
pub mod namespaces;
pub mod symbols;

use std::cell::RefCell;
use std::path::Path;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use tree_sitter::{Parser, Tree};

/// Extensions this backend claims. `.h` overlaps with `colab-lang-c`
/// deliberately — see the module docs.
const RELEVANT_EXTENSIONS: &[&str] = &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "h++", "h"];

pub(crate) fn is_relevant(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|ext| RELEVANT_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

thread_local! {
    /// Per-thread tree-sitter C++ parser. Reused across files so
    /// `Parser::new() + set_language()` only happens once per rayon
    /// worker.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_cpp::LANGUAGE.into())
            .expect("failed to load tree-sitter C++ grammar");
        p
    });
}

/// Parse `source` into a C++ syntax tree using the thread-local parser.
/// Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
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
];

impl LanguageBackend for CppBackend {
    fn lang(&self) -> &'static str {
        "cpp"
    }

    fn description(&self) -> &'static str {
        "C++ source rewrites via tree-sitter-cpp."
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
                "namespace",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(namespaces::NamespaceRename {
                from: target,
                to: replacement,
            })),
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
    fn claims_cpp_sources_and_headers() {
        for ext in ["cpp", "cc", "cxx", "hpp", "hh", "h"] {
            assert!(is_relevant(Path::new(&format!("a.{ext}"))), "{ext}");
        }
        assert!(!is_relevant(Path::new("a.rs")));
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
