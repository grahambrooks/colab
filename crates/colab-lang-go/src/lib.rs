//! Go backend for colab.
//!
//! Exposes a [`GoBackend`] that the binary registers with the
//! [`BackendRegistry`](colab_core::BackendRegistry), plus the underlying tree-sitter rewriters.

#[cfg(test)]
mod calls;
pub mod imports;
pub mod packages;
mod patterns;
pub mod struct_tags;
mod symbols;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about Go ([`colab_rewrite::language`]).
pub(crate) static LANG: SourceLanguage = SourceLanguage {
    namespace: "go",
    extensions: &["go"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &[
        "comment",
        "interpreted_string_literal_content",
        "raw_string_literal_content",
    ],
    calls: CallShape::Fields("call_expression"),
    qualifiers: Some(patterns::QUALIFIERS),
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
    colab_rewrite::language::pattern_capability(
        "Match code by shape: the target is Go code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `errors.Wrap($ERR, $MSG)`. An expression, statement, or declaration.",
    ),
    Capability {
        module: "package",
        description: "Rewrite the file's `package` clause. The import path is separate — pair with `go::import` for a package move.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Replace the matched package name with another.",
        }],
    },
];

impl LanguageBackend for GoBackend {
    fn lang(&self) -> &'static str {
        "go"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("import")
    }

    fn fence_names(&self) -> Vec<&'static str> {
        vec!["go", "golang"]
    }

    fn prose_ranges(&self, source: &str) -> Option<Vec<std::ops::Range<usize>>> {
        LANG.prose_ranges(source)
    }

    fn prose_locator(&self) -> Option<colab_core::ProseLocator> {
        Some(|source| LANG.prose_ranges(source))
    }

    fn drop_unused_import(&self, target: &str) -> Result<Box<dyn Operation>> {
        Ok(Box::new(imports::ImportDropUnused {
            target: target.to_string(),
        }))
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
        "Go source rewrites via tree-sitter."
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
                "struct_tag",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(struct_tags::TagReplace {
                from: target,
                to: replacement,
            })),
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
            (module, spec) => LANG.build_rule(self, module, spec),
        }
    }
}
