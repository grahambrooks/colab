//! Python backend for colab.
//!
//! `python::import` — rename / delete / ensure on import statements
//! (`import x.y`, `import x.y as z`, `from x.y import z`). Path
//! matching is segment-wise: `target = "x"` matches `x`, `x.y`, and
//! `from x.y import z` but never `xy` (substring) or `mod.x` (mid-path).

#[cfg(test)]
mod calls;
pub mod imports;
mod patterns;
mod symbols;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about Python ([`colab_rewrite::language`]).
pub static LANG: SourceLanguage = SourceLanguage {
    namespace: "python",
    extensions: &["py"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &["comment", "string_content"],
    // Keyword arguments travel with their names.
    calls: CallShape::Fields("call"),
    qualifiers: Some(patterns::QUALIFIERS),
    own: None,
};

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
    Capability {
        module: "call",
        description: "Rewrite a call using a template with $1/$2/$args/$func placeholders. Keyword arguments travel with their names.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace matched calls with the rendered template. Rename the function to stay idempotent.",
        }],
    },
    colab_rewrite::language::pattern_capability(
        "Match code by shape: the target is Python code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `self.assertEquals($A, $B)`. An expression, statement, or declaration.",
    ),
];

impl LanguageBackend for PythonBackend {
    fn lang(&self) -> &'static str {
        "python"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("import")
    }

    fn fence_names(&self) -> Vec<&'static str> {
        vec!["python", "py", "python3"]
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
        "Python source rewrites via tree-sitter-python."
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
            (module, spec) => LANG.build_rule(self, module, spec),
        }
    }
}
