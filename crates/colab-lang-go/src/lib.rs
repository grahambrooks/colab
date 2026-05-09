//! Go backend for colab.
//!
//! Exposes a [`GoBackend`] that the binary registers with the
//! [`BackendRegistry`], plus the underlying tree-sitter rewriters.

pub mod calls;
pub mod imports;
pub mod struct_tags;
pub mod symbols;

use colab_core::{
    ActionCapability, Capability, Error, LanguageBackend, Operation, Result, RuleSpec,
};

/// Plug-in entry point for the `go` namespace. Knows how to lower
/// `go::<module>` matches into concrete [`Operation`]s.
pub struct GoBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "import",
        description: "Rename, delete, or ensure a Go import path. Path matching is exact.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched import path with another.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the matched import line.",
            },
            ActionCapability {
                name: "ensure",
                description: "Idempotently add the import if it is missing.",
            },
        ],
    },
    Capability {
        module: "symbol",
        description: "Rename a top-level identifier and its in-file usages (functions, types, struct fields, method receivers). Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `identifier`/`type_identifier`/`field_identifier` whose text equals the target.",
        }],
    },
    Capability {
        module: "struct_tag",
        description: "Rewrite a `<key>:\"<value>\"` pair inside any struct field tag. Match string is `<key>:<value>` (no quotes around value). Other pairs in the same tag literal are left untouched.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Replace the matched key/value pair with the supplied one.",
        }],
    },
    Capability {
        module: "call",
        description: "Rewrite call expressions whose function text equals the target. Templates use `$1`/`$2` (positional args), `$args` (full arg list), `$func` (matched name), `$$` (literal $).",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace the entire call with the rendered template. Idempotency requires the template to rename the function.",
        }],
    },
];

impl LanguageBackend for GoBackend {
    fn lang(&self) -> &'static str {
        "go"
    }

    fn description(&self) -> &'static str {
        "Go source rewrites via tree-sitter."
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
            (
                "struct_tag",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(struct_tags::TagReplace {
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
                "go::{} does not support {:?}",
                other, spec
            ))),
        }
    }
}
