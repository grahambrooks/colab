//! TSX backend for colab, on `tree-sitter-typescript`'s TSX grammar.
//!
//! TypeScript with JSX: `tsx::pattern '<Button onClick={$H} />'` is an
//! element, and the rest is TypeScript as the `ts` backend reads it. The
//! TypeScript grammar cannot read JSX and the TSX grammar is a separate
//! one, so `.tsx` is a backend of its own. What does not depend on the
//! grammar is shared: the pattern contexts, `also` readings and modifier
//! keywords are `colab_lang_ts`'s, the rename kinds too, and the `import`
//! module is JavaScript's (`colab_lang_js::imports`), over this grammar.
//!
//! Applies to `.tsx`. `js` also claims `.tsx` (with the JavaScript
//! grammar), as it does `.ts`, so scripts written for it keep working.

mod patterns;

use colab_core::{ActionCapability, Capability, LanguageBackend, Operation, Result, RuleSpec};
use colab_rewrite::{CallShape, SourceLanguage};

/// What the shared `symbol` / `call` / `pattern` floor needs to know
/// about TSX ([`colab_rewrite::language`]).
pub static LANG: SourceLanguage = SourceLanguage {
    namespace: "tsx",
    extensions: &["tsx"],
    file_names: &[],
    pattern: patterns::language,
    rename_kinds: colab_lang_ts::RENAME_KINDS,
    // JSX text is what a reader sees, as a string's contents are.
    prose_kinds: &["comment", "string_fragment", "jsx_text"],
    calls: CallShape::Fields("call_expression"),
    qualifiers: Some(patterns::QUALIFIERS),
};

/// Plug-in entry point for the `tsx` namespace.
pub struct TsxBackend;

const CAPABILITIES: &[Capability] = &[
    Capability {
        module: "import",
        description: "Rewrite ES module specifiers (the string after `from`), in TSX files.",
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
        description: "Rename a TSX identifier (functions, variables, properties, types, components) and its in-file usages. Best-effort syntactic rename — verify with `--format diff` before `--write`.",
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
        "Match code by shape: the target is TSX code with metavariables — `$NAME` (one node), `$$$NAME` (zero or more), `$_` (wildcard) — e.g. `<Button onClick={$H} $$$A />` or `function $F(props: $P): JSX.Element { $$$B }`. JSX elements and attributes, type annotations, generics and member modifiers parse; a member pattern without modifiers matches one with any, and a rewrite keeps them.",
    ),
];

impl LanguageBackend for TsxBackend {
    fn lang(&self) -> &'static str {
        "tsx"
    }

    fn import_module(&self) -> Option<&'static str> {
        Some("import")
    }

    fn fence_names(&self) -> Vec<&'static str> {
        vec!["tsx"]
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
        "TSX (TypeScript with JSX) module specifiers, symbols, calls and patterns via tree-sitter-typescript's TSX grammar."
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
