//! ES module specifier rewriting.
//!
//! The trees (colab's own parser's, in the JavaScript, TypeScript or TSX
//! dialect) hold:
//! - `import_statement` with a `source` field → `string`
//! - `export_statement` with an optional `source` field → `string`
//!
//! We match the inner string value (without quotes) for exact
//! equality and replace those bytes only.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use colab_rewrite::SourceLanguage;
use colab_syntax::{Node, Tree};

/// `source` as colab's tree, by `lang`'s parser (colab's own, or
/// tree-sitter's read into colab's tree).
fn tree(lang: &SourceLanguage, source: &str) -> Option<Tree> {
    colab_rewrite::syntax::source_tree(&(lang.pattern)(), source)
}

/// The `import` module's operation for `spec` over `lang`'s files (a
/// grammar with JavaScript's module syntax: JavaScript's or TypeScript's),
/// or `spec` back when `module` is not `import` or the action is not one
/// it takes, for the backend's next arm.
pub fn build(
    lang: &'static SourceLanguage,
    module: &str,
    spec: colab_core::RuleSpec,
) -> std::result::Result<Box<dyn Operation>, colab_core::RuleSpec> {
    use colab_core::RuleSpec;
    match (module, spec) {
        (
            "import",
            RuleSpec::Replace {
                target,
                replacement,
            },
        ) => Ok(Box::new(SpecifierRename {
            lang,
            from: target,
            to: replacement,
        })),
        ("import", RuleSpec::Delete { target }) => Ok(Box::new(SpecifierDelete { lang, target })),
        ("import", RuleSpec::Ensure { target }) => Ok(Box::new(SpecifierEnsure { lang, target })),
        (_, spec) => Err(spec),
    }
}

/// For each `import_statement` / `export_statement` whose `source`
/// field is a string literal equal to `target`, invoke the visitor
/// with the enclosing statement and the inner-string byte range
/// (excluding the quotes).
fn for_each_matching_specifier<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
where
    F: FnMut(Node<'_>, usize, usize),
{
    for_each_specifier(tree, source, |node, inner_start, inner_end, value| {
        if value == target {
            visit(node, inner_start, inner_end);
        }
    });
}

/// Every `import_statement` / `export_statement` with a string-literal
/// `source`, with the inner-string byte range and its value.
fn for_each_specifier<F>(tree: &Tree, source: &str, mut visit: F)
where
    F: FnMut(Node<'_>, usize, usize, &str),
{
    tree.nodes().for_each(|node| {
        if matches!(node.kind_name(), "import_statement" | "export_statement")
            && let Some(specifier) = node.child_by_field("source")
            && specifier.kind_name() == "string"
            && let Some((inner_start, inner_end, value)) = string_inner_bytes(specifier, source)
        {
            visit(node, inner_start, inner_end, value);
        }
    });
}

/// Every module specifier in `source`, unquoted, for near-miss reporting.
/// Every name `source`'s module imports bind, which a qualified
/// reference (`ns.fn()`) may use: an `import`'s default, namespace and
/// named bindings (the alias, when there is one), and the names a
/// `require(…)` call is assigned to, destructured or not.
pub(crate) fn imported_names(source: &str) -> Vec<String> {
    imported_names_in(&crate::LANG, source)
}

/// Every name `source`'s module imports bind, as JavaScript's own
/// qualifier check reads them, for a grammar that shares JavaScript's
/// module syntax (TypeScript's), as a backend's `Qualifiers::imported`
/// calls it.
pub fn imported_names_in(lang: &SourceLanguage, source: &str) -> Vec<String> {
    let Some(tree) = tree(lang, source) else {
        return Vec::new();
    };
    let text = |n: Node<'_>| source[n.range()].to_string();
    let mut names = Vec::new();
    tree.nodes().for_each(|node| match node.kind_name() {
        "import_clause" => {
            for child in node.named_children() {
                match child.kind_name() {
                    "identifier" => names.push(text(child)),
                    "namespace_import" => {
                        names.extend(
                            child
                                .named_children()
                                .filter(|n| n.kind_name() == "identifier")
                                .map(text),
                        );
                    }
                    _ => {}
                }
            }
        }
        "import_specifier" => {
            if let Some(bound) = node
                .child_by_field("alias")
                .or_else(|| node.child_by_field("name"))
            {
                names.push(text(bound));
            }
        }
        "variable_declarator" => {
            let requires = node
                .child_by_field("value")
                .filter(|v| v.kind_name() == "call_expression")
                .and_then(|v| v.child_by_field("function"))
                .is_some_and(|f| &source[f.range()] == "require");
            let Some(name) = node.child_by_field("name").filter(|_| requires) else {
                return;
            };
            match name.kind_name() {
                "identifier" => names.push(text(name)),
                "object_pattern" => {
                    for part in name.named_children() {
                        match part.kind_name() {
                            "shorthand_property_identifier_pattern" => names.push(text(part)),
                            "pair_pattern" => {
                                if let Some(v) = part
                                    .child_by_field("value")
                                    .filter(|v| v.kind_name() == "identifier")
                                {
                                    names.push(text(v));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        _ => {}
    });
    names
}

pub fn specifiers(lang: &SourceLanguage, source: &str) -> Vec<String> {
    let Some(tree) = tree(lang, source) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for_each_specifier(&tree, source, |_, _, _, value| out.push(value.to_string()));
    out
}

/// Given a `string` node (with surrounding quotes), return the byte
/// range of its inner content and the string value.
fn string_inner_bytes<'a>(
    string_node: Node<'a>,
    source: &'a str,
) -> Option<(usize, usize, &'a str)> {
    // The inner text is a `string_fragment` child (or, empty, none): look
    // for it.
    for child in string_node.named_children() {
        if child.kind_name() == "string_fragment" {
            return Some((child.start(), child.end(), &source[child.range()]));
        }
    }
    // Fall back: strip the first/last byte (the quotes). Only safe
    // when the string contains no escapes; we guard above by
    // requiring an exact-match comparison and bailing if missing.
    let start = string_node.start();
    let end = string_node.end();
    if end > start + 1 {
        let inner_start = start + 1;
        let inner_end = end - 1;
        let text = source.get(inner_start..inner_end)?;
        Some((inner_start, inner_end, text))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SpecifierRename {
    pub lang: &'static SourceLanguage,
    pub from: String,
    pub to: String,
}

impl fmt::Display for SpecifierRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}::import \"{}\" -> \"{}\"",
            self.lang.namespace, self.from, self.to
        )
    }
}

impl Operation for SpecifierRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.lang.is_source(path)
    }

    fn apply(&self, source_code: &str) -> String {
        rename(self.lang, &self.from, &self.to, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        specifiers(self.lang, source)
    }
}

pub fn rename(lang: &SourceLanguage, from: &str, to: &str, source_code: &str) -> String {
    let Some(tree) = tree(lang, source_code) else {
        return source_code.to_string();
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for_each_matching_specifier(&tree, source_code, from, |_stmt, start, end| {
        edits.push((start, end, to.to_string()));
    });
    colab_rewrite::apply_edits(
        source_code,
        edits
            .into_iter()
            .map(|(start, end, text)| colab_rewrite::Edit::new(start, end, text))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SpecifierDelete {
    pub lang: &'static SourceLanguage,
    pub target: String,
}

impl fmt::Display for SpecifierDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}::import \"{}\" -> delete",
            self.lang.namespace, self.target
        )
    }
}

impl Operation for SpecifierDelete {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.lang.is_source(path)
    }

    fn apply(&self, source_code: &str) -> String {
        delete(self.lang, &self.target, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.target)
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.target)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        specifiers(self.lang, source)
    }
}

pub fn delete(lang: &SourceLanguage, target: &str, source_code: &str) -> String {
    let Some(tree) = tree(lang, source_code) else {
        return source_code.to_string();
    };
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for_each_matching_specifier(&tree, source_code, target, |stmt, _, _| {
        // `delete_lines` widens each range to whole lines.
        spans.push((stmt.start(), stmt.end()));
    });
    colab_rewrite::delete_lines(source_code, spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rename(from: &str, to: &str, source: &str) -> String {
        super::rename(&crate::LANG, from, to, source)
    }

    fn delete(target: &str, source: &str) -> String {
        super::delete(&crate::LANG, target, source)
    }

    fn specifiers(source: &str) -> Vec<String> {
        super::specifiers(&crate::LANG, source)
    }

    #[test]
    fn imported_names_cover_imports_and_require() {
        let src = "import a from 'a';\nimport * as ns from 'b';\nimport { c, d as e } from 'c';\nconst f = require('f');\nconst { g, h: k } = require('g');\nconst notImported = make();\n";
        assert_eq!(imported_names(src), ["a", "ns", "c", "e", "f", "g", "k"]);
    }

    #[test]
    fn observed_targets_lists_every_specifier() {
        let src = "import a from 'pkg/a';\nexport { b } from \"./b.js\";\n";
        assert_eq!(specifiers(src), vec!["pkg/a", "./b.js"]);
    }

    #[test]
    fn renames_default_import() {
        let src = "import old from 'old-mod';\n";
        let out = rename("old-mod", "new-mod", src);
        assert_eq!(out, "import old from 'new-mod';\n");
    }

    #[test]
    fn renames_named_import() {
        let src = "import { a, b } from \"old-mod\";\n";
        let out = rename("old-mod", "new-mod", src);
        assert_eq!(out, "import { a, b } from \"new-mod\";\n");
    }

    #[test]
    fn renames_export_from() {
        let src = "export { a } from 'old-mod';\n";
        let out = rename("old-mod", "new-mod", src);
        assert_eq!(out, "export { a } from 'new-mod';\n");
    }

    #[test]
    fn rename_does_not_match_substring() {
        let src = "import x from 'my-old-mod';\n";
        assert_eq!(rename("old-mod", "WRONG", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "import x from 'old-mod';\nimport y from 'other';\n";
        let once = rename("old-mod", "new-mod", src);
        let twice = rename("old-mod", "new-mod", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn deletes_matching_imports() {
        let src = "import a from 'os';\nimport b from 'old';\nimport c from 'fs';\n";
        let out = delete("old", src);
        assert!(!out.contains("'old'"));
        assert!(out.contains("'os'"));
        assert!(out.contains("'fs'"));
    }

    #[test]
    fn delete_is_idempotent() {
        let src = "import x from 'old';\n";
        let once = delete("old", src);
        let twice = delete("old", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn relevance_matches_js_ts_extensions() {
        let op = SpecifierRename {
            lang: &crate::LANG,
            from: "a".into(),
            to: "b".into(),
        };
        for ext in &["js", "mjs", "cjs", "jsx", "ts", "tsx"] {
            assert!(
                op.is_file_relevant(Path::new(&format!("foo.{ext}"))),
                "{ext} should be relevant"
            );
        }
        assert!(!op.is_file_relevant(Path::new("foo.go")));
        assert!(!op.is_file_relevant(Path::new("foo.py")));
    }
}

// ---------------------------------------------------------------------------
// Ensure
// ---------------------------------------------------------------------------

/// `Operation` that idempotently adds a side-effect import.
///
/// JS has no single canonical "import this module" form — a named import
/// needs to know *what* to bind — so `ensure` inserts the side-effect
/// form, `import '<target>';`. That is the shape used for polyfills,
/// stylesheets, and registration modules, which is where `ensure` is
/// actually useful. A module already imported in any form (named,
/// default, namespace, or side-effect) counts as present.
#[derive(Debug)]
pub struct SpecifierEnsure {
    pub lang: &'static SourceLanguage,
    pub target: String,
}

impl fmt::Display for SpecifierEnsure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}::import \"{}\" -> ensure",
            self.lang.namespace, self.target
        )
    }
}

impl Operation for SpecifierEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.lang.is_source(path)
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(self.lang, &self.target, source_code)
    }

    // No prefilter: `ensure` acts precisely when the target is absent.
}

/// Insert `import '<target>';` after the last existing import, or at the
/// top of the file when there is none.
pub fn ensure(lang: &SourceLanguage, target: &str, source_code: &str) -> String {
    let Some(tree) = tree(lang, source_code) else {
        return source_code.to_string();
    };

    let mut already_present = false;
    for_each_matching_specifier(&tree, source_code, target, |_, _, _| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    let root = tree.root();
    let mut insert_at = 0usize;
    for child in root.named_children() {
        if child.kind_name() == "import_statement" {
            let end = child.end();
            insert_at = source_code[end..]
                .find('\n')
                .map_or(source_code.len(), |p| end + p + 1);
        }
    }

    let insertion = format!("import '{target}';\n");
    let mut out = String::with_capacity(source_code.len() + insertion.len());
    out.push_str(&source_code[..insert_at]);
    out.push_str(&insertion);
    out.push_str(&source_code[insert_at..]);
    out
}

#[cfg(test)]
mod ensure_tests {
    fn ensure(target: &str, source: &str) -> String {
        super::ensure(&crate::LANG, target, source)
    }

    const SRC: &str = "import a from 'alpha';\nimport 'beta';\n\nconsole.log(a);\n";

    #[test]
    fn adds_a_side_effect_import_after_the_existing_ones() {
        let out = ensure("polyfill", SRC);
        assert!(out.contains("import 'polyfill';"), "got: {out}");
        let last_existing = out.find("import 'beta';").unwrap();
        let added = out.find("import 'polyfill';").unwrap();
        let body = out.find("console.log").unwrap();
        assert!(last_existing < added && added < body, "got: {out}");
    }

    #[test]
    fn is_a_noop_when_already_imported_in_any_form() {
        // Named/default import counts as present, not just side-effect.
        assert_eq!(ensure("alpha", SRC), SRC);
        assert_eq!(ensure("beta", SRC), SRC);
    }

    #[test]
    fn prepends_when_there_are_no_imports() {
        let src = "console.log(1);\n";
        let out = ensure("polyfill", src);
        assert!(out.starts_with("import 'polyfill';\n"), "got: {out}");
    }

    #[test]
    fn is_idempotent() {
        let once = ensure("polyfill", SRC);
        assert_eq!(ensure("polyfill", &once), once);
    }
}
