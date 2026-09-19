//! Swift backend for colab.
//!
//! Three operation families:
//!
//! - `swift::import` — rename, delete, ensure on `import` declarations.
//! - `swift::symbol` — rename an identifier and its in-file usages.
//! - `swift::call` — rewrite calls with a template.
//!
//! Swift has no package or namespace declaration in source — module
//! membership comes from the build system — so there is no
//! `swift::package` counterpart to `kotlin::package` or `java::package`.

pub mod calls;
pub mod imports;
pub mod symbols;

use std::cell::RefCell;
use std::path::Path;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use tree_sitter::{Parser, Tree};

/// Extensions this backend claims.
const RELEVANT_EXTENSIONS: &[&str] = &["swift"];

pub(crate) fn is_relevant(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|ext| RELEVANT_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

thread_local! {
    /// Per-thread tree-sitter Swift parser. Reused across files so
    /// `Parser::new() + set_language()` only happens once per rayon
    /// worker.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_swift::LANGUAGE.into())
            .expect("failed to load tree-sitter Swift grammar");
        p
    });
}

/// Parse `source` into a Swift syntax tree using the thread-local
/// parser. Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

/// Plug-in entry point for the `swift` namespace.
pub struct SwiftBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "import",
        description: "Rename, delete, or ensure an `import` declaration. Covers submodule (`UIKit.UIView`) and kind-qualified (`import class Old.Thing`) forms.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched module path with another.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the matched `import` line.",
            },
            ActionCapability {
                name: "ensure",
                description: "Idempotently add `import <target>` if missing.",
            },
        ],
    },
    Capability {
        module: "symbol",
        description: "Rename a type, function, property, or local and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `simple_identifier`/`type_identifier` whose text equals the target.",
        }],
    },
    Capability {
        module: "call",
        description: "Rewrite a call using a template with $1/$2/$args/$func placeholders. Argument labels travel with their values; trailing-closure calls are skipped.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace matched calls with the rendered template. Rename the function to stay idempotent.",
        }],
    },
];

impl LanguageBackend for SwiftBackend {
    fn lang(&self) -> &'static str {
        "swift"
    }

    fn description(&self) -> &'static str {
        "Swift source rewrites via tree-sitter-swift."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn build_rule(&self, module: &str, spec: RuleSpec) -> Result<Box<dyn Operation>> {
        match (module, spec) {
            (
                "import",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(imports::ImportRename {
                from: target,
                to: replacement,
            })),
            ("import", RuleSpec::Delete { target }) => {
                Ok(Box::new(imports::ImportDelete { target }))
            }
            ("import", RuleSpec::Ensure { target }) => {
                Ok(Box::new(imports::ImportEnsure { target }))
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
    fn claims_swift_sources() {
        assert!(is_relevant(Path::new("a.swift")));
        assert!(!is_relevant(Path::new("a.kt")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = SwiftBackend
            .build_rule("imort", RuleSpec::Delete { target: "x".into() })
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `swift::imort`"), "{msg}");
        assert!(msg.contains("did you mean `import`?"), "{msg}");
    }
}
