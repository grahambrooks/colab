//! Kotlin backend for colab.
//!
//! Four operation families:
//!
//! - `kotlin::import` — rename, delete, ensure on `import` directives.
//! - `kotlin::package` — rename the file's `package` header.
//! - `kotlin::symbol` — rename an identifier and its in-file usages.
//! - `kotlin::call` — rewrite calls with a template.

mod calls;
pub mod imports;
pub mod packages;
mod patterns;
mod symbols;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about Kotlin ([`colab_rewrite::language`]). `.kts` is the Kotlin
/// script form used by Gradle build files.
pub static LANG: SourceLanguage = SourceLanguage {
    namespace: "kotlin",
    extensions: &["kt", "kts"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &["line_comment", "block_comment", "string_content"],
    // A trailing lambda after the parentheses is left in place.
    calls: CallShape::Located {
        locate: calls::callee,
        through_arguments: true,
    },
    qualifiers: None,
    own: None,
};

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
    colab_rewrite::language::pattern_capability(
        "Match code by shape: the target is Kotlin code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `Log.d($TAG, $MSG)`. An expression, statement, or declaration.",
    ),
];

impl LanguageBackend for KotlinBackend {
    fn lang(&self) -> &'static str {
        "kotlin"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("import")
    }

    fn fence_names(&self) -> Vec<&'static str> {
        vec!["kotlin", "kt", "kts"]
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
        "Kotlin source rewrites via tree-sitter-kotlin-ng."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn syntax_tree(&self, input: colab_core::AstInput<'_>) -> Result<colab_core::SyntaxTree> {
        LANG.syntax_tree(input)
    }

    fn generalize(&self, before: &str, after: &str) -> Result<Vec<colab_core::Generalized>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn claims_kotlin_sources_and_scripts() {
        assert!(LANG.is_source(Path::new("a.kt")));
        assert!(LANG.is_source(Path::new("build.gradle.kts")));
        assert!(!LANG.is_source(Path::new("a.java")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = KotlinBackend
            .build_rule("imort", RuleSpec::Delete { target: "x".into() })
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `kotlin::imort`"), "{msg}");
        assert!(msg.contains("did you mean `import`?"), "{msg}");
    }
}
