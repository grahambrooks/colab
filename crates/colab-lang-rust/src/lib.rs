//! Rust backend for colab.
//!
//! Three operation families:
//!
//! - `rust::use` — rename / delete / ensure on a leading path prefix
//!   in `use` declarations.
//! - `rust::crate` — rename / delete on a `Cargo.toml` dependency
//!   (`[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`).

pub mod calls;
pub mod deps;
pub mod symbols;
pub mod uses;

use std::cell::RefCell;

use colab_core::{
    ActionCapability, Capability, Error, LanguageBackend, Operation, Result, RuleSpec,
};
use tree_sitter::{Parser, Tree};

thread_local! {
    /// Per-thread tree-sitter Rust parser. Reused across files so
    /// `Parser::new() + set_language()` only happens once per
    /// rayon worker.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_rust::LANGUAGE.into())
            .expect("failed to load tree-sitter Rust grammar");
        p
    });
}

/// Parse `source` into a Rust syntax tree using the thread-local
/// parser. Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

/// Plug-in entry point for the `rust` namespace.
pub struct RustBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "use",
        description: "Rewrite a leading path prefix in `use` declarations.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched path prefix with another.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove `use` declarations whose leading path matches.",
            },
            ActionCapability {
                name: "ensure",
                description: "Idempotently add a `use <target>;` declaration if missing.",
            },
        ],
    },
    Capability {
        module: "symbol",
        description: "Rename an identifier (functions, types, fields, shorthand fields) and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `identifier`/`type_identifier`/`field_identifier`/`shorthand_field_identifier` whose text equals the target.",
        }],
    },
    Capability {
        module: "crate",
        description: "Edit a Cargo.toml dependency.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Rename the matched key in [dependencies], [dev-dependencies], or [build-dependencies].",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the matched dependency entry.",
            },
        ],
    },
    Capability {
        module: "call",
        description: "Rewrite call expressions whose function text equals the target. Templates use `$1`/`$2` (positional args), `$args` (full arg list), `$func` (matched name), `$$` (literal $). Method calls (`x.foo()`) are excluded.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace the entire call with the rendered template. Idempotency requires the template to rename the function.",
        }],
    },
];

impl LanguageBackend for RustBackend {
    fn lang(&self) -> &'static str {
        "rust"
    }

    fn description(&self) -> &'static str {
        "Rust source rewrites via tree-sitter, plus Cargo.toml edits via toml_edit."
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
                "crate",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(deps::CrateRename {
                from: target,
                to: replacement,
            })),
            ("crate", RuleSpec::Delete { target }) => Ok(Box::new(deps::CrateDelete { target })),
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
            (other, spec) => Err(Error::UnsupportedOperation(format!(
                "rust::{} does not support {:?}",
                other, spec
            ))),
        }
    }
}
