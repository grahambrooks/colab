//! `tsx::pattern`: code-shaped structural rewrites with metavariables, on
//! the TSX grammar.
//!
//! All matching lives in [`colab_rewrite::pattern`], and the contexts,
//! `also` readings and modifier keywords are TypeScript's
//! ([`colab_lang_ts::pattern_language`]): only the grammar differs, so a
//! JSX element (`<Button onClick={$H} />`) parses as an expression.

use colab_rewrite::pattern::{PatternLanguage, Qualifiers};

pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_typescript::LANGUAGE_TSX.into(),
        ..colab_lang_ts::pattern_language()
    }
}

/// A rewrite that introduces `x.` with no import or `require` binding `x`
/// is a conflict, as in JavaScript; the imports are read with this
/// grammar, since a TSX file need not parse as TypeScript.
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

    const APP: &str = "import { Button } from './ui';\n\nexport function App(props: { label: string }): JSX.Element {\n  return <Button onClick={save} kind=\"primary\">{props.label}</Button>;\n}\n\nfunction Bare() {\n  return <Button onClick={load} />;\n}\n";

    #[test]
    fn a_typed_component_function_is_a_pattern() {
        let op = crate::LANG
            .compile_pattern(
                "function $F(props: $P): JSX.Element { $$$B }",
                PatternAction::Delete,
                &[],
            )
            .unwrap();
        let found = op.locate(APP).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].text.starts_with("function App"), "{found:?}");
    }

    #[test]
    fn a_jsx_element_is_a_pattern_and_a_rewrite_keeps_its_attributes() {
        // Both the self-closing element and the one with children.
        let op = crate::LANG
            .compile_pattern("<Button onClick={$H} $$$A />", PatternAction::Delete, &[])
            .unwrap();
        let found = op.locate(APP).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].text, "<Button onClick={load} />");
        // Rename the handler attribute; the other attributes and the
        // children stay as they were.
        assert_eq!(
            replace(
                "<Button onClick={$H} $$$A>$$$C</Button>",
                "<Button onPress={$H} $$$A>$$$C</Button>",
                APP
            ),
            APP.replace(
                "<Button onClick={save} kind=\"primary\">",
                "<Button onPress={save} kind=\"primary\">"
            )
        );
        // A rename of the component reaches its JSX tags.
        let renamed = crate::LANG.symbol_rename("Button", "Action").apply(APP);
        assert!(renamed.contains("<Action onClick={load} />"), "{renamed}");
        assert!(renamed.contains("</Action>"), "{renamed}");
    }

    #[test]
    fn a_member_pattern_without_modifiers_matches_one_with_them() {
        let src = "class Store {\n  private static async load(): Promise<void> {}\n  render() {\n    return <div />;\n  }\n}\n";
        assert_eq!(
            replace(
                "load(): Promise<void> {}",
                "reload(): Promise<void> {}",
                src
            ),
            src.replace("async load()", "async reload()")
        );
    }

    #[test]
    fn an_import_is_rewritten_in_a_tsx_file() {
        let op = colab_lang_js::imports::build(
            &crate::LANG,
            "import",
            colab_core::RuleSpec::Replace {
                target: "./ui".into(),
                replacement: "@app/ui".into(),
            },
        )
        .unwrap_or_else(|_| panic!("the import module builds"));
        assert!(
            op.apply(APP)
                .starts_with("import { Button } from '@app/ui';")
        );
    }
}
