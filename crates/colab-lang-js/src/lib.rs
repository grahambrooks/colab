//! JavaScript/TypeScript backend for colab.
//!
//! `js::import` — rewrite ES module specifiers in `import` and
//! `export … from` statements. Matching is exact-equality on the
//! quoted specifier, mirroring `go::import` semantics for safety.
//! Applies to `.js`, `.mjs`, `.cjs`, `.ts`, `.tsx`, and `.jsx` files
//! — the JavaScript grammar parses TypeScript module-syntax just
//! well enough for specifier rewriting; type-aware operations are
//! out of scope here: the `ts` backend (`colab-lang-ts`) parses
//! TypeScript with its own grammar.

#[cfg(test)]
mod calls;
pub mod imports;
mod patterns;
mod symbols;

pub use patterns::GLOBALS;
pub use symbols::RENAME_KINDS;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about JavaScript and TypeScript ([`colab_rewrite::language`]).
pub(crate) static LANG: SourceLanguage = SourceLanguage {
    namespace: "js",
    extensions: &["js", "mjs", "cjs", "jsx", "ts", "tsx"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: symbols::RENAME_KINDS,
    prose_kinds: &["comment", "string_fragment"],
    calls: CallShape::Fields("call_expression"),
    qualifiers: Some(patterns::QUALIFIERS),
};

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
    colab_rewrite::language::pattern_capability(
        "Match code by shape: the target is JavaScript/TypeScript code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `$A.indexOf($B) !== -1`. An expression, statement, or declaration. Parsed with the JavaScript grammar, so TypeScript-only syntax cannot appear in a pattern: use `ts::pattern` for that.",
    ),
];

impl LanguageBackend for JsBackend {
    fn lang(&self) -> &'static str {
        "js"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("import")
    }

    fn fence_names(&self) -> Vec<&'static str> {
        vec![
            "js",
            "javascript",
            "jsx",
            "mjs",
            "cjs",
            "ts",
            "typescript",
            "tsx",
        ]
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
        "JavaScript/TypeScript module-specifier rewrites via tree-sitter-javascript."
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
        match (module, imports::build(&LANG, module, spec)) {
            (_, Ok(op)) => Ok(op),
            (module, Err(spec)) => LANG.build_rule(self, module, spec),
        }
    }
}
