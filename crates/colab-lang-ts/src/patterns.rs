//! `ts::pattern`: code-shaped structural rewrites with metavariables, on
//! the TypeScript grammar.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies the grammar, the contexts that let a fragment parse, and the
//! modifier keywords a rewrite keeps.

use colab_rewrite::pattern::{PatternLanguage, Qualifiers};

/// `$` is a legal identifier character, so metavariables parse as
/// identifiers unchanged. Expressions, statements and declarations
/// (`interface`, `type`, `function`) parse on their own; the contexts
/// admit a class member (a method, a field with its modifiers) and a
/// `switch` case.
/// The `also` readings of [`crate::pattern_language`], public so `tsx` can
/// add JSX's.
pub const ALSO: [&str; 4] = [
    "({@@PATTERN@@});\n",
    "let _x: @@PATTERN@@;\n",
    "interface _I {\n@@PATTERN@@\n}\n",
    "class _C {\n@@PATTERN@@\n}\n",
];

pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        syntax: colab_rewrite::pattern::Syntax::Own(&colab_parse_js::TS_PARSER),
        expando: '$',
        contexts: &[
            "class C {\n@@PATTERN@@\n}\n",
            "switch (_x) {\n@@PATTERN@@\n}\n",
        ],
        // As in JavaScript: `debug: true` alone is a labelled statement; in
        // an object it is the property a pattern usually means. TypeScript
        // adds three: `Promise<$T>` alone is a comparison, and a type in an
        // annotation; `port: number` is also an interface's property, and
        // `count: number = 1;` a class's field.
        also: &ALSO,
        qualified_calls: None,
        // TSDoc documents the declaration after it, and a decorator belongs
        // to the member it decorates.
        leading: colab_rewrite::pattern::Leading {
            kinds: &["decorator"],
            doc_comments: &["/**"],
        },
        macro_contents: None,
        // The modifier keywords (`accessibility_modifier`, `static`, …) are
        // `modifier_tokens`, left out one by one; a decorator is the
        // member's sibling, not its child.
        modifiers: None,
        modifier_tokens: MODIFIER_TOKENS,
        method_calls: None,
        // `{ host }` is `{ host: host }`, as in JavaScript.
        shorthand_fields: Some(colab_rewrite::pattern::Shorthand::BRACES),
    }
}

/// A member's or function's modifiers: the named kinds (`private`,
/// `override`) and the keywords the grammar leaves anonymous. A pattern
/// that leaves them out matches the item with any, and a rewrite keeps
/// them.
pub const MODIFIER_TOKENS: &[&str] = &[
    "accessibility_modifier",
    "override_modifier",
    "static",
    "async",
    "readonly",
    "abstract",
    "declare",
    "get",
    "set",
    "*",
    "accessor",
];

/// A rewrite that introduces `x.` with no import or `require` binding `x`
/// is a conflict, as in JavaScript.
pub(crate) const QUALIFIERS: Qualifiers = Qualifiers {
    kind: "member_expression",
    field: "object",
    imported: imported_names,
    builtin: colab_lang_js::GLOBALS,
};

fn imported_names(source: &str) -> Vec<String> {
    colab_lang_js::imports::imported_names_in(&crate::LANG, source)
}

#[cfg(test)]
mod tests {
    use colab_core::Operation;
    use colab_rewrite::pattern::PatternAction;

    fn replace(pattern: &str, template: &str, source: &str) -> String {
        crate::LANG
            .compile_pattern(pattern, PatternAction::Replace(template.into()), &[])
            .unwrap()
            .apply(source)
    }

    #[test]
    fn a_typed_function_is_a_pattern() {
        let src = "export async function load(p: string): Promise<Config> {\n  return parse(p);\n}\nfunction sync(p: string): Config {\n  return parse(p);\n}\n";
        let op = crate::LANG
            .compile_pattern(
                "function $F($$$A): Promise<$T> { $$$B }",
                PatternAction::Delete,
                &[],
            )
            .unwrap();
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].text.starts_with("async function load"),
            "{found:?}"
        );
        // The generic's argument is a type, which the JavaScript grammar
        // could not read.
        assert_eq!(
            replace("Promise<$T>", "Async<$T>", src),
            src.replace("Promise<Config>", "Async<Config>")
        );
    }

    #[test]
    fn a_rewrite_keeps_the_modifiers_a_pattern_leaves_out() {
        let src = "class A {\n  private static async load(): void {}\n  static async load2(): void {}\n  *items() {}\n}\n";
        // Modifiers (`private`, `static`, `async`, `*`) are matched whether
        // or not the pattern spells them, and kept by the rewrite.
        assert_eq!(
            replace("load2(): void {}", "reload(): void {}", src),
            "class A {\n  private static async load(): void {}\n  static async reload(): void {}\n  *items() {}\n}\n"
        );
        assert_eq!(
            replace("private load(): void {}", "private reload(): void {}", src),
            "class A {\n  private static async reload(): void {}\n  static async load2(): void {}\n  *items() {}\n}\n"
        );
        assert_eq!(
            replace("items() {}", "entries() {}", src),
            "class A {\n  private static async load(): void {}\n  static async load2(): void {}\n  *entries() {}\n}\n"
        );
        // A pattern without `private` matches the private member, and a
        // metavariable name reads past the modifiers too.
        assert_eq!(
            replace("load(): void {}", "x(): void {}", src),
            "class A {\n  private static async x(): void {}\n  static async load2(): void {}\n  *items() {}\n}\n"
        );
        assert_eq!(
            replace("$M(): void {}", "$M(): Promise<void> {}", src),
            "class A {\n  private static async load(): Promise<void> {}\n  static async load2(): Promise<void> {}\n  *items() {}\n}\n"
        );
        // A field's name is not a modifier: `static readonly` stays.
        assert_eq!(
            replace(
                "count: number = $V",
                "count: number = 2",
                "class B {\n  static readonly count: number = 1;\n}\n"
            ),
            "class B {\n  static readonly count: number = 2;\n}\n"
        );
        // A template that spells the modifiers is used as written.
        assert_eq!(
            replace("load2(): void {}", "static reload(): void {}", src),
            "class A {\n  private static async load(): void {}\n  static reload(): void {}\n  *items() {}\n}\n"
        );
    }

    #[test]
    fn an_interface_is_a_pattern() {
        let src = "interface Config {\n  name: string;\n  port: number;\n}\ntype Alias = Config;\n";
        assert_eq!(
            replace(
                "interface Config { $$$M }",
                "interface Settings { $$$M }",
                src
            ),
            "interface Settings {\n  name: string;\n  port: number;\n}\ntype Alias = Config;\n"
        );
        let field = crate::LANG
            .compile_pattern("port: number", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(
            field.apply(src),
            "interface Config {\n  name: string;\n}\ntype Alias = Config;\n"
        );
    }
}
