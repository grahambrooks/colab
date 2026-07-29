//! Kotlin backend for colab.
//!
//! Four operation families:
//!
//! - `kotlin::import` — rename, delete, ensure on `import` directives.
//! - `kotlin::package` — rename the file's `package` header.
//! - `kotlin::symbol` — rename an identifier and its in-file usages.
//! - `kotlin::call` — rewrite calls with a template.

pub mod calls;
pub mod imports;
pub mod packages;
pub mod symbols;

use std::cell::RefCell;
use std::path::Path;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use tree_sitter::{Parser, Tree};

/// Extensions this backend claims. `.kts` is the Kotlin script form
/// used by Gradle build files.
const RELEVANT_EXTENSIONS: &[&str] = &["kt", "kts"];

pub(crate) fn is_relevant(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|ext| RELEVANT_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

thread_local! {
    /// Per-thread tree-sitter Kotlin parser. Reused across files so
    /// `Parser::new() + set_language()` only happens once per rayon
    /// worker.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_kotlin_ng::LANGUAGE.into())
            .expect("failed to load tree-sitter Kotlin grammar");
        p
    });
}

/// Parse `source` into a Kotlin syntax tree using the thread-local
/// parser. Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

/// Plug-in entry point for the `kotlin` namespace.
pub struct KotlinBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "import",
        description: "Rename, delete, or ensure an `import` directive. For an aliased import the target is the qualified name, not the alias.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched import name with another.",
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
        module: "package",
        description: "Rewrite the file's `package` header.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Replace the matched package name with another.",
        }],
    },
    Capability {
        module: "symbol",
        description: "Rename a class, function, property, or local and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `identifier` whose text equals the target.",
        }],
    },
    Capability {
        module: "call",
        description: "Rewrite a call using a template with $1/$2/$args/$func placeholders. Trailing-lambda calls are skipped.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace matched calls with the rendered template. Rename the function to stay idempotent.",
        }],
    },
];

impl LanguageBackend for KotlinBackend {
    fn lang(&self) -> &'static str {
        "kotlin"
    }

    fn description(&self) -> &'static str {
        "Kotlin source rewrites via tree-sitter-kotlin-ng."
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
                "package",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(packages::PackageRename {
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
    fn claims_kotlin_sources_and_scripts() {
        assert!(is_relevant(Path::new("a.kt")));
        assert!(is_relevant(Path::new("build.gradle.kts")));
        assert!(!is_relevant(Path::new("a.java")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = KotlinBackend
            .build_rule(
                "imort",
                RuleSpec::Delete {
                    target: "x".into(),
                },
            )
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `kotlin::imort`"), "{msg}");
        assert!(msg.contains("did you mean `import`?"), "{msg}");
    }
}
