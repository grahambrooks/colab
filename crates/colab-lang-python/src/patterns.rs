//! `python::pattern`: code-shaped structural rewrites with metavariables.
//!
//! All matching lives in [`colab_rewrite::pattern`]; this module only
//! supplies Python's grammar, expando character, and the contexts that
//! let a fragment that is not a valid Python *file* on its own parse.

use colab_rewrite::pattern::{PatternLanguage, Qualifiers};

/// `$` is not a Python identifier character, so metavariables are parsed
/// as `µ`. Expressions and statements parse on their own; the one context
/// is a function body, for a single-line statement that needs one.
pub(crate) fn language() -> PatternLanguage {
    PatternLanguage {
        grammar: tree_sitter_python::LANGUAGE.into(),
        expando: 'µ',
        contexts: &["def _():\n    @@PATTERN@@\n"],
        // In an annotation `Optional[str]` is a `generic_type`, not the
        // `subscript` it is as an expression; match both. `debug=True` is
        // an assignment on its own but a keyword argument in a call, and
        // `"k": v` is code only as a dict entry.
        also: &[
            "def _(x: @@PATTERN@@): pass\n",
            "f(@@PATTERN@@)\n",
            "{@@PATTERN@@}\n",
        ],
        qualified_calls: None,
    }
}

/// A rewrite that introduces `X.` with no import binding `X` is a
/// conflict (see [`Qualifiers`]).
pub(crate) const QUALIFIERS: Qualifiers = Qualifiers {
    kind: "attribute",
    field: "object",
    imported: crate::imports::imported_names,
    builtin: PYTHON_BUILTINS,
};

/// Builtins used as qualifiers (`str.join`, `dict.fromkeys`) need no import.
const PYTHON_BUILTINS: &[&str] = &[
    "bool",
    "bytearray",
    "bytes",
    "complex",
    "dict",
    "float",
    "frozenset",
    "int",
    "list",
    "object",
    "range",
    "set",
    "str",
    "tuple",
    "type",
];

#[cfg(test)]
mod tests {
    use colab_core::{Error, Operation};
    use colab_rewrite::pattern::PatternAction;

    #[test]
    fn python_qualifiers_count_imports_aliases_and_builtins() {
        let op = crate::LANG
            .compile_pattern(
                "open($P).read()",
                PatternAction::Replace("pathlib.Path($P).read_text()".into()),
                &[],
            )
            .unwrap();
        let before = "def f(p):\n    return open(p).read()\n";
        let after = op.apply(before);
        assert_eq!(op.conflicts(before, &after).len(), 1);
        for header in [
            "import pathlib\n",
            "from x import pathlib\n",
            "import y as pathlib\n",
            "from z import *\n",
        ] {
            let b = format!("{header}{before}");
            assert!(op.conflicts(&b, &op.apply(&b)).is_empty(), "{header}");
        }
        let builtin = crate::LANG
            .compile_pattern(
                "join($A)",
                PatternAction::Replace("str.join(\"\", $A)".into()),
                &[],
            )
            .unwrap();
        let src = "x = join(parts)\n";
        assert!(builtin.conflicts(src, &builtin.apply(src)).is_empty());
    }

    #[test]
    fn rewrites_call_with_two_captures() {
        let op = crate::LANG
            .compile_pattern(
                "self.assertEquals($A, $B)",
                PatternAction::Replace("self.assertEqual($A, $B)".into()),
                &[],
            )
            .unwrap();
        let src = "class T(TestCase):\n    def test_add(self):\n        self.assertEquals(add(2, 2), 4)\n";
        let out = op.apply(src);
        assert_eq!(
            out,
            "class T(TestCase):\n    def test_add(self):\n        self.assertEqual(add(2, 2), 4)\n"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        assert_eq!(
            op.to_string(),
            "python::pattern 'self.assertEquals($A, $B)' -> 'self.assertEqual($A, $B)'"
        );
    }

    #[test]
    fn multi_capture_spans_all_arguments() {
        let op = crate::LANG
            .compile_pattern(
                "print($$$ARGS)",
                PatternAction::Replace("logger.info($$$ARGS)".into()),
                &[],
            )
            .unwrap();
        let src = "def f(a, b):\n    print(\"sum\", a + b)\n";
        let out = op.apply(src);
        assert_eq!(out, "def f(a, b):\n    logger.info(\"sum\", a + b)\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn deletes_a_standalone_statement_line() {
        let op = crate::LANG
            .compile_pattern("pdb.set_trace()", PatternAction::Delete, &[])
            .unwrap();
        let src = "def f(x):\n    pdb.set_trace()\n    return x\n";
        let out = op.apply(src);
        assert_eq!(out, "def f(x):\n    return x\n");
        assert_eq!(op.apply(&out), out, "idempotent");
    }

    #[test]
    fn expression_and_statement_patterns_compile() {
        let src = "def f():\n    y = foo.bar(1, 2)\n    return y\n";
        for pattern in ["foo.bar($A, $B)", "return $X"] {
            let op = crate::LANG
                .compile_pattern(pattern, PatternAction::Delete, &[])
                .unwrap();
            let found = op.locate(src).unwrap();
            assert_eq!(found.len(), 1, "{pattern}");
        }
    }

    #[test]
    fn a_bare_identifier_compiles() {
        // The selector must be the pattern's own identifier, not the first
        // identifier of whichever context it was parsed in.
        let op = crate::LANG
            .compile_pattern("legacy_name", PatternAction::Delete, &[])
            .unwrap();
        assert_eq!(op.prefilter(), Some("legacy_name"));
        let src = "def f():\n    y = legacy_name.bar(1, 2)\n    return y\n";
        let found = op.locate(src).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "legacy_name");
    }

    #[test]
    fn invalid_pattern_is_an_unsupported_operation() {
        let err = crate::LANG
            .compile_pattern("def (", PatternAction::Delete, &[])
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedOperation(_)), "{err}");
        assert!(err.to_string().contains("python::pattern"), "{err}");
    }
}
