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

pub mod calls;
pub mod requires;
pub mod symbols;

use std::cell::RefCell;
use std::path::Path;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use tree_sitter::{Parser, Tree};

/// Extensions this backend claims.
const RELEVANT_EXTENSIONS: &[&str] = &["rb", "rake", "gemspec", "ru"];

/// Extension-less Ruby files that are conventional enough to claim by name.
const RELEVANT_FILENAMES: &[&str] = &["Rakefile", "Gemfile", "Guardfile", "Capfile"];

pub(crate) fn is_relevant(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str())
        && RELEVANT_EXTENSIONS.contains(&ext)
    {
        return true;
    }
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|name| RELEVANT_FILENAMES.contains(&name))
        .unwrap_or(false)
}

thread_local! {
    /// Per-thread tree-sitter Ruby parser. Reused across files so
    /// `Parser::new() + set_language()` only happens once per rayon
    /// worker.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_ruby::LANGUAGE.into())
            .expect("failed to load tree-sitter Ruby grammar");
        p
    });
}

/// Parse `source` into a Ruby syntax tree using the thread-local parser.
/// Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

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
];

impl LanguageBackend for RubyBackend {
    fn lang(&self) -> &'static str {
        "ruby"
    }

    fn description(&self) -> &'static str {
        "Ruby source rewrites via tree-sitter-ruby."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
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
            (other, spec) => Err(self.unsupported(other, &spec)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_ruby_sources_and_conventional_filenames() {
        assert!(is_relevant(Path::new("a.rb")));
        assert!(is_relevant(Path::new("tasks.rake")));
        assert!(is_relevant(Path::new("Rakefile")));
        assert!(is_relevant(Path::new("some/dir/Gemfile")));
        assert!(!is_relevant(Path::new("a.rs")));
        assert!(!is_relevant(Path::new("README")));
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
