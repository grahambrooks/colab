//! Java backend for colab.
//!
//! Two operation families:
//!
//! - `java::import` — rename, delete, ensure on `import` declarations
//!   (handles both regular and `import static`).
//! - `java::package` — rename the file's `package` declaration.

pub mod imports;
pub mod packages;
pub mod symbols;

use std::cell::RefCell;

use colab_core::{
    ActionCapability, Capability, Error, LanguageBackend, Operation, Result, RuleSpec,
};
use tree_sitter::{Parser, Tree};

thread_local! {
    /// Per-thread tree-sitter Java parser. Reused across files so
    /// `Parser::new() + set_language()` only happens once per
    /// rayon worker.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_java::LANGUAGE.into())
            .expect("failed to load tree-sitter Java grammar");
        p
    });
}

/// Parse `source` into a Java syntax tree using the thread-local
/// parser. Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

/// Plug-in entry point for the `java` namespace.
pub struct JavaBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "import",
        description: "Rename, delete, or ensure a Java `import` declaration.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched import name with another.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the matched import line.",
            },
            ActionCapability {
                name: "ensure",
                description: "Idempotently add `import <target>;` if missing.",
            },
        ],
    },
    Capability {
        module: "package",
        description: "Rewrite the file's `package` declaration.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Replace the matched package name with another.",
        }],
    },
    Capability {
        module: "symbol",
        description: "Rename a class, method, field, or local identifier and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `identifier`/`type_identifier` whose text equals the target.",
        }],
    },
];

impl LanguageBackend for JavaBackend {
    fn lang(&self) -> &'static str {
        "java"
    }

    fn description(&self) -> &'static str {
        "Java source rewrites via tree-sitter-java."
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
            (other, spec) => Err(Error::UnsupportedOperation(format!(
                "java::{} does not support {:?}",
                other, spec
            ))),
        }
    }
}
