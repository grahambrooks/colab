//! Python backend for colab.
//!
//! `python::import` — rename / delete / ensure on import statements
//! (`import x.y`, `import x.y as z`, `from x.y import z`). Path
//! matching is segment-wise: `target = "x"` matches `x`, `x.y`, and
//! `from x.y import z` but never `xy` (substring) or `mod.x` (mid-path).

pub mod calls;
pub mod imports;
pub mod patterns;
pub mod symbols;

use std::cell::RefCell;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use tree_sitter::{Parser, Tree};

thread_local! {
    /// Per-thread tree-sitter Python parser. Reused across files
    /// so `Parser::new() + set_language()` only happens once per
    /// rayon worker.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("failed to load tree-sitter Python grammar");
        p
    });
}

/// Parse `source` into a Python syntax tree using the
/// thread-local parser. Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

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
    Capability {
        module: "pattern",
        description: "Match code by shape: the target is Python code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `self.assertEquals($A, $B)`. An expression, statement, or declaration.",
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

impl LanguageBackend for PythonBackend {
    fn lang(&self) -> &'static str {
        "python"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("import")
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
        "Python source rewrites via tree-sitter-python."
    }

    fn capabilities(&self) -> &'static [Capability] {
        CAPABILITIES
    }

    fn syntax_tree(&self, input: colab_core::AstInput<'_>) -> Result<colab_core::SyntaxTree> {
        patterns::syntax_tree(input)
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
