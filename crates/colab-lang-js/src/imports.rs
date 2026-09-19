//! ES module specifier rewriting.
//!
//! Tree-sitter-javascript represents:
//! - `import_statement` with a `source` field → `string`
//! - `export_statement` with an optional `source` field → `string`
//!
//! We match the inner string value (without quotes) for exact
//! equality and replace those bytes only.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Tree};

const RELEVANT_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "jsx", "ts", "tsx"];

pub(crate) fn is_relevant(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|ext| RELEVANT_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

/// For each `import_statement` / `export_statement` whose `source`
/// field is a string literal equal to `target`, invoke the visitor
/// with the enclosing statement and the inner-string byte range
/// (excluding the quotes).
fn for_each_matching_specifier<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
where
    F: FnMut(Node<'_>, usize, usize),
{
    colab_rewrite::visit_all(tree, |node| {
        if matches!(node.kind(), "import_statement" | "export_statement")
            && let Some(specifier) = node.child_by_field_name("source")
            && specifier.kind() == "string"
            && let Some((inner_start, inner_end, value)) = string_inner_bytes(specifier, source)
            && value == target
        {
            visit(node, inner_start, inner_end);
        }
    });
}

/// Given a `string` node (with surrounding quotes), return the byte
/// range of its inner content and the string value.
fn string_inner_bytes<'a>(
    string_node: Node<'a>,
    source: &'a str,
) -> Option<(usize, usize, &'a str)> {
    // tree-sitter-javascript wraps the inner text in either
    // `string_fragment` children or matches the literal directly. The
    // safest approach: look for a `string_fragment` named child.
    for i in 0..string_node.named_child_count() {
        let Some(child) = string_node.named_child(i as u32) else {
            break;
        };
        if child.kind() == "string_fragment"
            && let Ok(text) = child.utf8_text(source.as_bytes())
        {
            return Some((child.start_byte(), child.end_byte(), text));
        }
    }
    // Fall back: strip the first/last byte (the quotes). Only safe
    // when the string contains no escapes; we guard above by
    // requiring an exact-match comparison and bailing if missing.
    let start = string_node.start_byte();
    let end = string_node.end_byte();
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
    pub from: String,
    pub to: String,
}

impl fmt::Display for SpecifierRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "js::import \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for SpecifierRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.from)
    }
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
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
    pub target: String,
}

impl fmt::Display for SpecifierDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "js::import \"{}\" -> delete", self.target)
    }
}

impl Operation for SpecifierDelete {
    fn is_file_relevant(&self, path: &Path) -> bool {
        is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        delete(&self.target, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.target)
    }
}

pub fn delete(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for_each_matching_specifier(&tree, source_code, target, |stmt, _, _| {
        // `delete_lines` widens each range to whole lines.
        spans.push((stmt.start_byte(), stmt.end_byte()));
    });
    colab_rewrite::delete_lines(source_code, spans)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            from: "a".into(),
            to: "b".into(),
        };
        for ext in &["js", "mjs", "cjs", "jsx", "ts", "tsx"] {
            assert!(
                op.is_file_relevant(Path::new(&format!("foo.{}", ext))),
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
    pub target: String,
}

impl fmt::Display for SpecifierEnsure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "js::import \"{}\" -> ensure", self.target)
    }
}

impl Operation for SpecifierEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(&self.target, source_code)
    }

    // No prefilter: `ensure` acts precisely when the target is absent.
}

/// Insert `import '<target>';` after the last existing import, or at the
/// top of the file when there is none.
pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut already_present = false;
    for_each_matching_specifier(&tree, source_code, target, |_, _, _| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    let root = tree.root_node();
    let mut insert_at = 0usize;
    for i in 0..root.named_child_count() {
        let Some(child) = root.named_child(i as u32) else {
            break;
        };
        if child.kind() == "import_statement" {
            let end = child.end_byte();
            insert_at = source_code[end..]
                .find('\n')
                .map(|p| end + p + 1)
                .unwrap_or(source_code.len());
        }
    }

    let insertion = format!("import '{}';\n", target);
    let mut out = String::with_capacity(source_code.len() + insertion.len());
    out.push_str(&source_code[..insert_at]);
    out.push_str(&insertion);
    out.push_str(&source_code[insert_at..]);
    out
}

#[cfg(test)]
mod ensure_tests {
    use super::*;

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
