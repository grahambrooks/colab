//! Rust backend for colab.
//!
//! Three operation families:
//!
//! - `rust::use` — rename / delete / ensure on a leading path prefix
//!   in `use` declarations.
//! - `rust::crate` — rename / delete on a `Cargo.toml` dependency
//!   (`[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`).

#[cfg(test)]
mod calls;
pub mod deps;
mod patterns;
mod symbols;
pub mod uses;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about Rust ([`colab_rewrite::language`]). `Cargo.toml` belongs to
/// `rust::crate` alone ([`deps`]).
pub(crate) static LANG: SourceLanguage = SourceLanguage {
    namespace: "rust",
    extensions: &["rs"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &["line_comment", "block_comment", "string_content"],
    calls: CallShape::Fields("call_expression"),
    qualifiers: None,
};

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
    colab_rewrite::language::pattern_capability(
        "Match code by shape: the target is Rust code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `$MAP.get($KEY).is_some()`. An expression, statement, or declaration.",
    ),
];

impl LanguageBackend for RustBackend {
    fn lang(&self) -> &'static str {
        "rust"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("use")
    }

    fn fence_names(&self) -> Vec<&'static str> {
        vec!["rust", "rs"]
    }

    fn prose_ranges(&self, source: &str) -> Option<Vec<std::ops::Range<usize>>> {
        LANG.prose_ranges(source)
    }

    fn prose_locator(&self) -> Option<colab_core::ProseLocator> {
        Some(|source| LANG.prose_ranges(source))
    }

    fn build_rule_where(
        &self,
        module: &str,
        spec: RuleSpec,
        constraints: &[colab_core::Constraint],
    ) -> Result<Box<dyn Operation>> {
        LANG.build_rule_where(self, module, spec, constraints)
    }

    fn description(&self) -> &'static str {
        "Rust source rewrites via tree-sitter, plus Cargo.toml edits via toml_edit."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn syntax_tree(&self, input: colab_core::AstInput<'_>) -> Result<colab_core::SyntaxTree> {
        LANG.syntax_tree(input)
    }

    fn generalize(&self, before: &str, after: &str) -> Result<colab_core::Generalized> {
        LANG.generalize(before, after)
    }

    fn declarations(&self, source: &str) -> Option<Vec<colab_core::symbols::Declaration>> {
        Some(LANG.declarations(source))
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
            (module, spec) => LANG.build_rule(self, module, spec),
        }
    }
}
