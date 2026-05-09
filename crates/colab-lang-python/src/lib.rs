//! Python backend for colab.
//!
//! `python::import` — rename / delete / ensure on import statements
//! (`import x.y`, `import x.y as z`, `from x.y import z`). Path
//! matching is segment-wise: `target = "x"` matches `x`, `x.y`, and
//! `from x.y import z` but never `xy` (substring) or `mod.x` (mid-path).

pub mod imports;
pub mod symbols;

use std::cell::RefCell;

use colab_core::{
    ActionCapability, Capability, Error, LanguageBackend, Operation, Result, RuleSpec,
};
use tree_sitter::{Parser, Tree};

thread_local! {
    /// Per-thread tree-sitter Python parser. Reused across files
    /// so `Parser::new() + set_language()` only happens once per
    /// rayon worker.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("failed to load tree-sitter Python grammar");
        p
    });
}

/// Parse `source` into a Python syntax tree using the
/// thread-local parser. Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

/// Plug-in entry point for the `python` namespace.
pub struct PythonBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "import",
        description: "Rename, delete, or ensure a Python import (segment-wise prefix match).",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched import path prefix with another.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove import statements whose path matches.",
            },
            ActionCapability {
                name: "ensure",
                description: "Idempotently add `import <target>` if no existing import covers it.",
            },
        ],
    },
    Capability {
        module: "symbol",
        description: "Rename a Python identifier and its in-file usages (functions, classes, variables). Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `identifier` whose text equals the target.",
        }],
    },
];

impl LanguageBackend for PythonBackend {
    fn lang(&self) -> &'static str {
        "python"
    }

    fn description(&self) -> &'static str {
        "Python source rewrites via tree-sitter-python."
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
            (other, spec) => Err(Error::UnsupportedOperation(format!(
                "python::{} does not support {:?}",
                other, spec
            ))),
        }
    }
}
