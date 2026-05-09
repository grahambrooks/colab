//! JavaScript/TypeScript backend for colab.
//!
//! `js::import` — rewrite ES module specifiers in `import` and
//! `export … from` statements. Matching is exact-equality on the
//! quoted specifier, mirroring `go::import` semantics for safety.
//! Applies to `.js`, `.mjs`, `.cjs`, `.ts`, `.tsx`, and `.jsx` files
//! — the JavaScript grammar parses TypeScript module-syntax just
//! well enough for specifier rewriting; type-aware operations are
//! out of scope.

pub mod imports;
pub mod symbols;

use std::cell::RefCell;

use colab_core::{
    ActionCapability, Capability, Error, LanguageBackend, Operation, Result, RuleSpec,
};
use tree_sitter::{Parser, Tree};

thread_local! {
    /// Per-thread tree-sitter JavaScript parser. Reused across
    /// files so `Parser::new() + set_language()` only happens once
    /// per rayon worker. The JavaScript grammar parses TypeScript
    /// module syntax well enough for our specifier and identifier
    /// rewrites; type-aware ops are out of scope.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_javascript::LANGUAGE.into())
            .expect("failed to load tree-sitter JavaScript grammar");
        p
    });
}

/// Parse `source` into a JS/TS syntax tree using the
/// thread-local parser. Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

/// Plug-in entry point for the `js` namespace.
pub struct JsBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "import",
        description: "Rewrite ES module specifiers (the string after `from`).",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched specifier with another.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove `import`/`export-from` statements with a matching specifier.",
            },
        ],
    },
    Capability {
        module: "symbol",
        description: "Rename a JS/TS identifier (functions, variables, properties) and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `identifier`/`property_identifier`/`shorthand_property_identifier` whose text equals the target.",
        }],
    },
];

impl LanguageBackend for JsBackend {
    fn lang(&self) -> &'static str {
        "js"
    }

    fn description(&self) -> &'static str {
        "JavaScript/TypeScript module-specifier rewrites via tree-sitter-javascript."
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
            ) => Ok(Box::new(imports::SpecifierRename {
                from: target,
                to: replacement,
            })),
            ("import", RuleSpec::Delete { target }) => {
                Ok(Box::new(imports::SpecifierDelete { target }))
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
                "js::{} does not support {:?}",
                other, spec
            ))),
        }
    }
}
