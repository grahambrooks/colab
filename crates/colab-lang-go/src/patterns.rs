//! `go::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Go's grammar, expando character, and the contexts that let an
//! expression or statement — not a valid Go *file* on its own — parse.

use colab_rewrite::pattern::{PatternLanguage, Qualifiers};

/// `$` cannot start a Go identifier, so metavariables are parsed as `µ`.
/// The contexts are tried in order: a statement or expression inside a
/// function body, a top-level declaration, then a `switch` case
/// (`case $X: $$$B`), the values a `return` lists (`nil, $E`), and a
/// parameter (`words ...string`). The parameter context declares a type
/// before the slot, so a bare name is never also read as a parameter's type.
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        syntax: colab_rewrite::pattern::Syntax::Own(&colab_parse_go::PARSER),
        expando: 'µ',
        contexts: &[
            "package p\nfunc _() {\n@@PATTERN@@\n}\n",
            "package p\n@@PATTERN@@\n",
            "package p\nfunc _() {\nswitch {\n@@PATTERN@@\n}\n}\n",
            "package p\nfunc _() {\nreturn @@PATTERN@@\n}\n",
            "package p\ntype _t int\nfunc _(@@PATTERN@@) {}\n",
        ],
        also: &[],
        qualified_calls: None,
        // Go defines a declaration's doc comment as the comments directly
        // before it, with no blank line between (go.dev/doc/comment), and
        // `Leading` takes only those.
        leading: colab_rewrite::pattern::Leading {
            kinds: &[],
            doc_comments: &["//", "/*"],
        },
        macro_contents: None,
        modifiers: None,
        shorthand_fields: None,
        modifier_tokens: &[],
        method_calls: None,
    }
}

/// A rewrite that introduces `X.` with no import binding `X` is a
/// conflict (see [`Qualifiers`]).
pub(crate) const QUALIFIERS: Qualifiers = Qualifiers {
    kind: "selector_expression",
    field: "operand",
    imported: crate::imports::imported_names,
    builtin: &[],
};

#[cfg(test)]
mod tests {
    use colab_core::{Error, Operation};
    use colab_rewrite::pattern::PatternAction;

    fn replace(pattern: &str, template: &str, src: &str) -> String {
        let op = crate::LANG
            .compile_pattern(pattern, PatternAction::Replace(template.into()), &[])
            .unwrap();
        let out = op.apply(src);
        assert_eq!(op.apply(&out), out, "idempotent");
        out
    }

    #[test]
    fn returned_values_and_a_parameter_are_patterns() {
        let src = "package p\nfunc f() (int, error) {\n\treturn 0, errors.New(\"no\")\n}\n";
        assert_eq!(
            replace("0, errors.New($M)", "-1, errors.New($M)", src),
            "package p\nfunc f() (int, error) {\n\treturn -1, errors.New(\"no\")\n}\n"
        );
        let src = "package p\nfunc join(words ...string) string { return \"\" }\n";
        assert_eq!(
            replace("$W ...string", "$W ...any", src),
            "package p\nfunc join(words ...any) string { return \"\" }\n"
        );
        // The parameter context does not make a bare name a type as well.
        let op = crate::LANG
            .compile_pattern("Order", PatternAction::Delete, &[])
            .unwrap();
        let src = "package p\nfunc f(o Order) {}\n";
        assert!(op.locate(src).unwrap().is_empty());
    }

    #[test]
    fn rewrites_errors_wrap_to_fmt_errorf() {
        let op = crate::LANG
            .compile_pattern(
                "errors.Wrap($ERR, $MSG)",
                PatternAction::Replace("fmt.Errorf(\"%s: %w\", $MSG, $ERR)".into()),
                &[],
            )
            .unwrap();
        let src = "package main\n\nfunc f() error {\n\treturn errors.Wrap(err, \"read\")\n}\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "package main\n\nfunc f() error {\n\treturn fmt.Errorf(\"%s: %w\", \"read\", err)\n}\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "go::pattern 'errors.Wrap($ERR, $MSG)' -> 'fmt.Errorf(\"%s: %w\", $MSG, $ERR)'"
        );
    }

    #[test]
    fn a_qualifier_nothing_imports_is_a_conflict() {
        let op = crate::LANG
            .compile_pattern(
                "errors.Wrap($E, $M)",
                PatternAction::Replace("fmt.Errorf($M+\": %w\", $E)".into()),
                &[],
            )
            .unwrap();
        let before = "package p\n\nimport \"github.com/pkg/errors\"\n\nfunc f() error {\n\treturn errors.Wrap(err, \"x\")\n}\n";
        let after = op.apply(before);
        let conflicts = op.conflicts(before, &after);
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert!(conflicts[0].contains("`fmt.`"));

        // Imported (here by an alias-free `fmt`), or already used: no conflict.
        let imported = after.replace(
            "import \"github.com/pkg/errors\"",
            "import (\n\t\"fmt\"\n\t\"github.com/pkg/errors\"\n)",
        );
        assert!(op.conflicts(before, &imported).is_empty());
        let used = before.replace("return", "fmt.Println()\n\treturn");
        assert!(op.conflicts(&used, &op.apply(&used)).is_empty());
        // `err.Error()` qualifies by a capture already in the file.
        let wrap = crate::LANG
            .compile_pattern(
                "errors.Wrap($E, $M)",
                PatternAction::Replace("errors.New($E.Error())".into()),
                &[],
            )
            .unwrap();
        assert!(wrap.conflicts(before, &wrap.apply(before)).is_empty());
    }

    #[test]
    fn a_template_that_does_not_parse_explains_the_dead_rule() {
        let op = crate::LANG
            .compile_pattern("f($$$A)", PatternAction::Replace("g($$$A,,)".into()), &[])
            .unwrap();
        let src = "package p\n\nfunc main() {\n\tf(1, 2)\n}\n";
        assert_eq!(op.apply(src), src);
        assert!(op.why_unchanged(src).unwrap().contains("would not parse"));
        assert_eq!(op.why_unchanged("package p\n"), None);
    }

    #[test]
    fn a_skipped_rewrite_names_the_match_that_breaks_the_file() {
        // A template that breaks every site names the first.
        let bad = crate::LANG
            .compile_pattern("f($$$A)", PatternAction::Replace("g($$$A,,)".into()), &[])
            .unwrap();
        let src = "package p\n\nfunc main() {\n\tx := 1\n\tf(1, 2)\n\tf(3)\n}\n";
        let why = bad.why_unchanged(src).unwrap();
        assert!(why.contains("at line 5, column 2"), "{why}");
    }

    #[test]
    fn top_level_declaration_pattern_compiles() {
        let op = crate::LANG
            .compile_pattern("var $NAME = $VALUE", PatternAction::Delete, &[])
            .unwrap();
        let out = op.apply("package p\n\nvar debug = true\n\nfunc main() {}\n");
        // One blank line stays between the neighbours, not both.
        assert_eq!(out, "package p\n\nfunc main() {}\n");
    }

    #[test]
    fn a_switch_case_is_a_pattern() {
        let src = "package p\n\nfunc f(x int) {\n\tswitch x {\n\tcase 1:\n\t\tg()\n\tcase 2:\n\t\th()\n\t}\n}\n";
        let del = crate::LANG
            .compile_pattern("case 1: $$$B", PatternAction::Delete, &[])
            .unwrap();
        let out = del.apply(src);
        assert_eq!(
            out,
            "package p\n\nfunc f(x int) {\n\tswitch x {\n\tcase 2:\n\t\th()\n\t}\n}\n"
        );
        assert_eq!(del.apply(&out), out, "idempotent");
        // A case owns the newline that ends it; a rewrite keeps it.
        let swap = crate::LANG
            .compile_pattern(
                "case 1: $$$B",
                PatternAction::Replace("case 3: $$$B".into()),
                &[],
            )
            .unwrap();
        assert_eq!(
            swap.apply(src),
            "package p\n\nfunc f(x int) {\n\tswitch x {\n\tcase 3:\n\t\tg()\n\tcase 2:\n\t\th()\n\t}\n}\n"
        );
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = crate::LANG
            .compile_pattern("func (", PatternAction::Delete, &[])
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("go::pattern"), "{err}");
    }
}
