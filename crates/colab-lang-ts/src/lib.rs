//! TypeScript backend for colab, on `tree-sitter-typescript`.
//!
//! The `js` backend also runs over `.ts` files, with the JavaScript
//! grammar, which reads type annotations, interfaces, generics and member
//! modifiers as errors. `ts::` parses them: `ts::pattern 'private $F() {}'`
//! means a method, and an item pattern written without modifiers matches
//! a member with any (`private static async`). The `import` module is
//! JavaScript's (`colab_lang_js::imports`), over this grammar.
//!
//! Applies to `.ts`, `.mts` and `.cts`. `.tsx` needs the TSX grammar, so
//! it is the `tsx` backend's (`colab-lang-tsx`), which takes this crate's
//! pattern setup, modifiers and rename kinds: one backend has one grammar.

mod patterns;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

pub use patterns::{ALSO as PATTERN_ALSO, MODIFIER_TOKENS};

/// TypeScript's pattern setup (contexts, `also` readings, modifiers), for
/// a backend on another TypeScript grammar to take with its own: `tsx`.
pub fn pattern_language() -> colab_rewrite::pattern::PatternLanguage {
    patterns::language()
}

/// Identifier kinds TypeScript adds to JavaScript's: a type's name.
pub const RENAME_KINDS: &[&str] = &[
    "identifier",
    "property_identifier",
    "shorthand_property_identifier",
    "type_identifier",
];

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about TypeScript ([`colab_rewrite::language`]).
pub static LANG: SourceLanguage = SourceLanguage {
    namespace: "ts",
    extensions: &["ts", "mts", "cts"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: RENAME_KINDS,
    prose_kinds: &["comment", "string_fragment"],
    calls: CallShape::Fields("call_expression"),
    qualifiers: Some(patterns::QUALIFIERS),
};

/// Plug-in entry point for the `ts` namespace.
pub struct TsBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "import",
        description: "Rewrite ES module specifiers (the string after `from`), in TypeScript files.",
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
        description: "Rename a TypeScript identifier (functions, variables, properties, types) and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
        actions: &[ActionCapability {
            name: "replace",
            description: "Rewrite every `identifier`/`property_identifier`/`shorthand_property_identifier`/`type_identifier` whose text equals the target.",
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
        "Match code by shape: the target is TypeScript code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `function $F($$$A): Promise<$T> { $$$B }`. Type annotations, interfaces, generics and member modifiers parse; a member pattern without modifiers matches one with any (`private static`), and a rewrite keeps them.",
    ),
];

impl LanguageBackend for TsBackend {
    fn lang(&self) -> &'static str {
        "ts"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("import")
    }

    fn fence_names(&self) -> Vec<&'static str> {
        vec!["ts", "typescript", "mts", "cts"]
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
        "TypeScript module specifiers, symbols, calls and patterns via tree-sitter-typescript."
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
        match (module, colab_lang_js::imports::build(&LANG, module, spec)) {
            (_, Ok(op)) => Ok(op),
            (module, Err(spec)) => LANG.build_rule(self, module, spec),
        }
    }
}
