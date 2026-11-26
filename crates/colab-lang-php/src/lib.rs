//! PHP backend for colab.
//!
//! Four operation families:
//!
//! - `php::use` — rename, delete, ensure on `use` declarations,
//!   including the `use function` and `use const` forms.
//! - `php::namespace` — rename a `namespace` declaration.
//! - `php::symbol` — rename a name node and its in-file usages.
//! - `php::call` — rewrite plain, static (`::`), and member (`->`)
//!   calls with a template.

mod calls;
pub mod namespaces;
mod patterns;
mod symbols;
pub mod uses;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about PHP ([`colab_rewrite::language`]).
pub static LANG: SourceLanguage = SourceLanguage {
    namespace: "php",
    extensions: &["php", "phtml"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &["comment", "string_content"],
    calls: CallShape::Prefixed(calls::CALL_KINDS),
    qualifiers: None,
    own: None,
};

/// `source` as colab's PHP tree (`colab-parse-php`).
pub(crate) fn tree(source: &str) -> Option<colab_syntax::Tree> {
    colab_parse_php::parse(source).ok()
}

/// Plug-in entry point for the `php` namespace.
pub struct PhpBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "use",
        description: "Rename, delete, or ensure a `use` declaration, including `use function` and `use const`. Grouped imports (`use A\\{B, C};`) are deliberately not matched.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched use name with another.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the matched `use` line.",
            },
            ActionCapability {
                name: "ensure",
                description: "Idempotently add `use <target>;` if missing.",
            },
        ],
    },
    Capability {
        module: "namespace",
        description: "Rewrite a `namespace` declaration, statement or block form.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Replace the matched namespace name with another.",
        }],
    },
    Capability {
        module: "symbol",
        description: "Rename a class, function, method, or constant name and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `name` node whose text equals the target.",
        }],
    },
    Capability {
        module: "call",
        description: "Rewrite a call using a template with $1/$2/$args/$func placeholders. Targets include the `::` or `->` prefix.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace matched calls with the rendered template. Rename the function to stay idempotent.",
        }],
    },
    colab_rewrite::language::pattern_capability(
        "Match code by shape: the target is PHP code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `$this->assertEquals($A, $B)`. An expression, statement, or declaration. Metavariables are upper-case, so `$x` stays a PHP variable; a metavariable cannot stand where only a variable may (the left of `=`).",
    ),
];

impl LanguageBackend for PhpBackend {
    fn lang(&self) -> &'static str {
        "php"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("use")
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
        "PHP source rewrites via colab's own PHP parser."
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
                "namespace",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(namespaces::NamespaceRename {
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
    fn claims_php_sources() {
        assert!(LANG.is_source(Path::new("a.php")));
        assert!(LANG.is_source(Path::new("a.phtml")));
        assert!(!LANG.is_source(Path::new("a.rs")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = PhpBackend
            .build_rule("uses", RuleSpec::Delete { target: "x".into() })
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `php::uses`"), "{msg}");
        assert!(msg.contains("did you mean `use`?"), "{msg}");
    }
}
