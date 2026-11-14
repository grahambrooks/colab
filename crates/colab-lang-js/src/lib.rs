//! JavaScript/TypeScript backend for colab.
//!
//! `js::import` — rewrite ES module specifiers in `import` and
//! `export … from` statements. Matching is exact-equality on the
//! quoted specifier, mirroring `go::import` semantics for safety.
//! Applies to `.js`, `.mjs`, `.cjs`, `.ts`, `.tsx`, and `.jsx` files
//! — the JavaScript grammar parses TypeScript module-syntax just
//! well enough for specifier rewriting; type-aware operations are
//! out of scope.

pub mod calls;
pub mod imports;
pub mod patterns;
pub mod symbols;

use std::cell::RefCell;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use tree_sitter::{Parser, Tree};

thread_local! {
    /// Per-thread tree-sitter JavaScript parser. Reused across
    /// files so `Parser::new() + set_language()` only happens once
    /// per rayon worker. The JavaScript grammar parses TypeScript
    /// module syntax well enough for our specifier and identifier
    /// rewrites; type-aware ops are out of scope.
    static PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_javascript::LANGUAGE.into())
            .expect("failed to load tree-sitter JavaScript grammar");
        p
    });
}

/// Parse `source` into a JS/TS syntax tree using the
/// thread-local parser. Returns `None` if tree-sitter cannot parse.
pub(crate) fn parse(source: &str) -> Option<Tree> {
    PARSER.with_borrow_mut(|p| p.parse(source, None))
}

/// Plug-in entry point for the `js` namespace.
pub struct JsBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "import",
        description: "Rewrite ES module specifiers (the string after `from`).",
        actions: &[
            ActionCapability {
                name: "replace",
                description: "Replace the matched specifier with another.",
            },
            ActionCapability {
                name: "delete",
                description: "Remove `import`/`export-from` statements with a matching specifier.",
            },
            ActionCapability {
                name: "ensure",
                description: "Idempotently add a side-effect import (`import '<target>';`) if the module is not imported in any form.",
            },
        ],
    },
    Capability {
        module: "symbol",
        description: "Rename a JS/TS identifier (functions, variables, properties) and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `identifier`/`property_identifier`/`shorthand_property_identifier` whose text equals the target.",
        }],
    },
    Capability {
        module: "call",
        description: "Rewrite a call using a template with $1/$2/$args/$func placeholders. Targets include the receiver, e.g. `mod.old`.",
        actions: &[ActionCapability {
            name: "replace_call",
            description: "Replace matched calls with the rendered template. Rename the function to stay idempotent.",
        }],
    },
    Capability {
        module: "pattern",
        description: "Match code by shape: the target is JavaScript/TypeScript code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `$A.indexOf($B) !== -1`. An expression, statement, or declaration. Parsed with the JavaScript grammar, so TypeScript-only syntax cannot appear in a pattern.",
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

impl LanguageBackend for JsBackend {
    fn lang(&self) -> &'static str {
        "js"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("import")
    }

    fn prose_ranges(&self, source: &str) -> Option<Vec<std::ops::Range<usize>>> {
        let kinds = ["comment", "string_fragment"];
        crate::parse(source).map(|tree| colab_rewrite::ranges_of_kinds(&tree, &kinds))
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
        "JavaScript/TypeScript module-specifier rewrites via tree-sitter-javascript."
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
            ) => Ok(Box::new(imports::SpecifierRename {
                from: target,
                to: replacement,
            })),
            ("import", RuleSpec::Delete { target }) => {
                Ok(Box::new(imports::SpecifierDelete { target }))
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
            ("import", RuleSpec::Ensure { target }) => {
                Ok(Box::new(imports::SpecifierEnsure { target }))
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
