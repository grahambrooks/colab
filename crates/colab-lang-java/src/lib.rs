//! Java backend for colab.
//!
//! Two operation families:
//!
//! - `java::import` — rename, delete, ensure on `import` declarations
//!   (handles both regular and `import static`).
//! - `java::package` — rename the file's `package` declaration.

#[cfg(test)]
mod calls;
pub mod imports;
pub mod packages;
mod patterns;
mod symbols;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about Java ([`colab_rewrite::language`]).
pub static LANG: SourceLanguage = SourceLanguage {
    namespace: "java",
    extensions: &["java"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &[
        "line_comment",
        "block_comment",
        "string_fragment",
        "multiline_string_fragment",
    ],
    calls: CallShape::Prefixed("method_invocation"),
    qualifiers: None,
    own: None,
};

/// `source` as colab's Java tree (`colab-parse-java`).
pub(crate) fn tree(source: &str) -> Option<colab_syntax::Tree> {
    colab_parse_java::parse(source).ok()
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
    Capability {
        module: "call",
        description: "Rewrite a method invocation using a template with $1/$2/$args/$func placeholders. Targets include the receiver, e.g. `Old.run`.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace matched calls with the rendered template. Rename the function to stay idempotent.",
        }],
    },
    colab_rewrite::language::pattern_capability(
        "Match code by shape: the target is Java code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `Assert.assertEquals($A, $B)`. An expression, statement, or declaration.",
    ),
];

impl LanguageBackend for JavaBackend {
    fn lang(&self) -> &'static str {
        "java"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("import")
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
        "Java source rewrites on colab's own Java parser."
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
