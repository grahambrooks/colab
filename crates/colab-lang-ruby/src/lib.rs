//! Ruby backend for colab.
//!
//! Three operation families:
//!
//! - `ruby::require` — rename, delete, ensure on `require` and
//!   `require_relative`. Ruby has no import statement, so these are
//!   matched as method calls with a string-literal argument.
//! - `ruby::symbol` — rename an identifier or constant and its in-file
//!   usages. This also covers `module` and `class` names, which are
//!   constants; Ruby has no separate namespace declaration to rename.
//! - `ruby::call` — rewrite parenthesised calls with a template.

mod calls;
mod patterns;
pub mod requires;
mod symbols;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about Ruby ([`colab_rewrite::language`]).
pub(crate) static LANG: SourceLanguage = SourceLanguage {
    namespace: "ruby",
    extensions: &["rb", "rake", "gemspec", "ru"],
    // Extension-less Ruby files that are conventional enough to claim by name.
    file_names: &["Rakefile", "Gemfile", "Guardfile", "Capfile"],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &["comment", "string_content", "heredoc_content"],
    calls: CallShape::Located {
        locate: calls::paren_call,
        through_arguments: false,
    },
    qualifiers: None,
};

/// Plug-in entry point for the `ruby` namespace.
pub struct RubyBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "require",
        description: "Rename, delete, or ensure a `require` or `require_relative`. Matches the quoted path; computed requires never match.",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched require path, preserving the quote style.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove the matched require line.",
            },
            ActionCapability {
                name: "ensure",
                description: "Idempotently add `require '<target>'` if missing.",
            },
        ],
    },
    Capability {
        module: "symbol",
        description: "Rename a method, local, class, or module name and its in-file usages. Covers `module`/`class` names, which are constants. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `identifier`/`constant` whose text equals the target.",
        }],
    },
    Capability {
        module: "call",
        description: "Rewrite a parenthesised call using a template with $1/$2/$args/$func placeholders. Targets include the receiver, e.g. `Old.run`.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace matched calls with the rendered template. Rename the method to stay idempotent.",
        }],
    },
    colab_rewrite::language::pattern_capability(
        "Match code by shape: the target is Ruby code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `$X.should == $Y`. An expression, statement, or declaration.",
    ),
];

impl LanguageBackend for RubyBackend {
    fn lang(&self) -> &'static str {
        "ruby"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("require")
    }

    fn fence_names(&self) -> Vec<&'static str> {
        vec!["ruby", "rb"]
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
        "Ruby source rewrites via tree-sitter-ruby."
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
                "require",
                RuleSpec::Replace {
                    target,
                    replacement,
                },
            ) => Ok(Box::new(requires::RequireRename {
                from: target,
                to: replacement,
            })),
            ("require", RuleSpec::Delete { target }) => {
                Ok(Box::new(requires::RequireDelete { target }))
            }
            ("require", RuleSpec::Ensure { target }) => {
                Ok(Box::new(requires::RequireEnsure { target }))
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
    fn claims_ruby_sources_and_conventional_filenames() {
        assert!(LANG.is_source(Path::new("a.rb")));
        assert!(LANG.is_source(Path::new("tasks.rake")));
        assert!(LANG.is_source(Path::new("Rakefile")));
        assert!(LANG.is_source(Path::new("some/dir/Gemfile")));
        assert!(!LANG.is_source(Path::new("a.rs")));
        assert!(!LANG.is_source(Path::new("README")));
    }

    #[test]
    fn unknown_module_is_rejected_with_alternatives() {
        let err = RubyBackend
            .build_rule("requires", RuleSpec::Delete { target: "x".into() })
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown module `ruby::requires`"), "{msg}");
        assert!(msg.contains("did you mean `require`?"), "{msg}");
    }
}
