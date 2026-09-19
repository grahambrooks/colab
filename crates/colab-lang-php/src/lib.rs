//! PHP backend for colab.
//!
//! Four operation families:
//!
//! - `php::use` — rename, delete, ensure on `use` declarations,
//!   including the `use function` and `use const` forms.
//! - `php::namespace` — rename a `namespace` declaration.
//! - `php::symbol` — rename a name node and its in-file usages.
//! - `php::call` — rewrite plain, static (`::`), and member (`->`)
//!   calls with a template.

pub mod calls;
pub mod namespaces;
pub mod symbols;
pub mod uses;

use std::cell::RefCell;
use std::path::Path;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use tree_sitter::{Parser, Tree};

/// Extensions this backend claims.
const RELEVANT_EXTENSIONS: &[&str] = &["php", "phtml"];

pub(crate) fn is_relevant(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|ext| RELEVANT_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

thread_local! {
    /// Per-thread tree-sitter PHP parser. Reused across files so
    /// `Parser::new() + set_language()` only happens once per rayon
    /// worker.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_php::LANGUAGE_PHP.into())
            .expect("failed to load tree-sitter PHP grammar");
        p
    });
}

/// Parse `source` into a PHP syntax tree using the thread-local parser.
/// Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

/// Plug-in entry point for the `php` namespace.
pub struct PhpBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "use",
        description: "Rename, delete, or ensure a `use` declaration, including `use function` and `use const`. Grouped imports (`use A\\{B, C};`) are deliberately not matched.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched use name with another.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the matched `use` line.",
            },
            ActionCapability {
                name: "ensure",
                description: "Idempotently add `use <target>;` if missing.",
            },
        ],
    },
    Capability {
        module: "namespace",
        description: "Rewrite a `namespace` declaration, statement or block form.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Replace the matched namespace name with another.",
        }],
    },
    Capability {
        module: "symbol",
        description: "Rename a class, function, method, or constant name and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `name` node whose text equals the target.",
        }],
    },
    Capability {
        module: "call",
        description: "Rewrite a call using a template with $1/$2/$args/$func placeholders. Targets include the `::` or `->` prefix.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace matched calls with the rendered template. Rename the function to stay idempotent.",
        }],
    },
];

impl LanguageBackend for PhpBackend {
    fn lang(&self) -> &'static str {
        "php"
    }

    fn description(&self) -> &'static str {
        "PHP source rewrites via tree-sitter-php."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>> {
        match (module, spec) {
            (
                "use",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(uses::UseRename {
                from: target,
                to: replacement,
            })),
            ("use", RuleSpec::Delete { target }) => Ok(Box::new(uses::UseDelete { target })),
            ("use", RuleSpec::Ensure { target }) => Ok(Box::new(uses::UseEnsure { target })),
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
    fn claims_php_sources() {
        assert!(is_relevant(Path::new("a.php")));
        assert!(is_relevant(Path::new("a.phtml")));
        assert!(!is_relevant(Path::new("a.rs")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = PhpBackend
            .build_rule("uses", RuleSpec::Delete { target: "x".into() })
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `php::uses`"), "{msg}");
        assert!(msg.contains("did you mean `use`?"), "{msg}");
    }
}
