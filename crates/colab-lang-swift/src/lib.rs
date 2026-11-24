//! Swift backend for colab.
//!
//! Three operation families:
//!
//! - `swift::import` — rename, delete, ensure on `import` declarations.
//! - `swift::symbol` — rename an identifier and its in-file usages.
//! - `swift::call` — rewrite calls with a template.
//!
//! Swift has no package or namespace declaration in source — module
//! membership comes from the build system — so there is no
//! `swift::package` counterpart to `kotlin::package` or `java::package`.

mod calls;
pub mod imports;
mod patterns;
mod symbols;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about Swift ([`colab_rewrite::language`]).
pub(crate) static LANG: SourceLanguage = SourceLanguage {
    namespace: "swift",
    extensions: &["swift"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &[
        "comment",
        "multiline_comment",
        "line_str_text",
        "multi_line_str_text",
    ],
    // A trailing closure after the parentheses is left in place.
    calls: CallShape::Located {
        locate: calls::callee,
        through_arguments: true,
    },
    qualifiers: None,
};

/// Plug-in entry point for the `swift` namespace.
pub struct SwiftBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "import",
        description: "Rename, delete, or ensure an `import` declaration. Covers submodule (`UIKit.UIView`) and kind-qualified (`import class Old.Thing`) forms.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched module path with another.",
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
        module: "symbol",
        description: "Rename a type, function, property, or local and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `simple_identifier`/`type_identifier` whose text equals the target.",
        }],
    },
    Capability {
        module: "call",
        description: "Rewrite a call using a template with $1/$2/$args/$func placeholders. Argument labels travel with their values; trailing-closure calls are skipped.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace matched calls with the rendered template. Rename the function to stay idempotent.",
        }],
    },
    colab_rewrite::language::pattern_capability(
        "Match code by shape: the target is Swift code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `XCTAssertEqual($A, $B)`. An expression, statement, or declaration.",
    ),
];

impl LanguageBackend for SwiftBackend {
    fn lang(&self) -> &'static str {
        "swift"
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
        "Swift source rewrites via tree-sitter-swift."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn syntax_tree(&self, input: colab_core::AstInput<'_>) -> Result<colab_core::SyntaxTree> {
        LANG.syntax_tree(input)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn claims_swift_sources() {
        assert!(LANG.is_source(Path::new("a.swift")));
        assert!(!LANG.is_source(Path::new("a.kt")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = SwiftBackend
            .build_rule("imort", RuleSpec::Delete { target: "x".into() })
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `swift::imort`"), "{msg}");
        assert!(msg.contains("did you mean `import`?"), "{msg}");
    }
}
