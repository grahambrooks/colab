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
pub mod patterns;
pub mod symbols;
pub mod uses;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use tree_sitter::Tree;

/// Parse `source` into a Rust syntax tree, through the shared parse
/// cache (`colab_rewrite::parse_cache`): a text the rules before have
/// already parsed is not parsed again. `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    colab_rewrite::parse_cache::parse(&patterns::language().grammar, source)
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
    Capability {
        module: "pattern",
        description: "Match code by shape: the target is Rust code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `$MAP.get($KEY).is_some()`. An expression, statement, or declaration.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace each match with the template, substituting `$NAME` / `$$$NAME` bindings.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove each match; a match alone on its line takes the line with it.",
            },
        ],
    },
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
        let kinds = ["line_comment", "block_comment", "string_content"];
        crate::parse(source).map(|tree| colab_rewrite::ranges_of_kinds(&tree, &kinds))
    }

    fn prose_locator(&self) -> Option<colab_core::ProseLocator> {
        Some(|source| Self.prose_ranges(source))
    }

    fn build_rule_where(
        &self,
        module: &str,
        spec: RuleSpec,
        constraints: &[colab_core::Constraint],
    ) -> Result<Box<dyn Operation>> {
        colab_rewrite::pattern::build_rule_where(self, module, spec, constraints, |p, a, c| {
            Ok(Box::new(patterns::PatternOp::compile(p, a, c)?))
        })
    }

    fn description(&self) -> &'static str {
        "Rust source rewrites via tree-sitter, plus Cargo.toml edits via toml_edit."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn syntax_tree(&self, input: colab_core::AstInput<'_>) -> Result<colab_core::SyntaxTree> {
        patterns::syntax_tree(input)
    }

    fn declarations(&self, source: &str) -> Option<Vec<colab_core::symbols::Declaration>> {
        Some(colab_rewrite::declarations(
            &patterns::language().grammar,
            source,
        ))
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
            (
                "pattern",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(patterns::PatternOp::compile(
                &target,
                colab_rewrite::pattern::PatternAction::Replace(replacement),
                &[],
            )?)),
            ("pattern", RuleSpec::Delete { target }) => Ok(Box::new(patterns::PatternOp::compile(
                &target,
                colab_rewrite::pattern::PatternAction::Delete,
                &[],
            )?)),
            (other, spec) => Err(self.unsupported(other, &spec)),
        }
    }
}
