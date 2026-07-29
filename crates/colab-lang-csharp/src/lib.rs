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

pub mod calls;
pub mod namespaces;
pub mod symbols;
pub mod usings;

use std::cell::RefCell;
use std::path::Path;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use tree_sitter::{Parser, Tree};

/// Extensions this backend claims. `.csx` is the C# scripting form.
const RELEVANT_EXTENSIONS: &[&str] = &["cs", "csx"];

pub(crate) fn is_relevant(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|ext| RELEVANT_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

thread_local! {
    /// Per-thread tree-sitter C# parser. Reused across files so
    /// `Parser::new() + set_language()` only happens once per rayon
    /// worker.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_c_sharp::LANGUAGE.into())
            .expect("failed to load tree-sitter C# grammar");
        p
    });
}

/// Parse `source` into a C# syntax tree using the thread-local parser.
/// Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

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
];

impl LanguageBackend for CSharpBackend {
    fn lang(&self) -> &'static str {
        "csharp"
    }

    fn description(&self) -> &'static str {
        "C# source rewrites via tree-sitter-c-sharp."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
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
    fn claims_cs_sources() {
        assert!(is_relevant(Path::new("a.cs")));
        assert!(is_relevant(Path::new("a.csx")));
        assert!(!is_relevant(Path::new("a.c")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = CSharpBackend
            .build_rule(
                "usng",
                RuleSpec::Delete {
                    target: "x".into(),
                },
            )
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `csharp::usng`"), "{msg}");
        assert!(msg.contains("did you mean `using`?"), "{msg}");
    }
}
