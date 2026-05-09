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
use tree_sitter::{Node, Parser, Tree, TreeCursor};

const RELEVANT_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "jsx", "ts", "tsx"];

pub(crate) fn is_relevant(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|ext| RELEVANT_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

fn parse_js(source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_javascript::LANGUAGE.into())
        .expect("failed to load tree-sitter JavaScript grammar");
    parser.parse(source, None)
}

/// For each `import_statement` / `export_statement` whose `source`
/// field is a string literal equal to `target`, invoke the visitor
/// with the enclosing statement and the inner-string byte range
/// (excluding the quotes).
fn for_each_matching_specifier<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
where
    F: FnMut(Node<'_>, usize, usize),
{
    let mut cursor = tree.walk();
    walk(&mut cursor, source, target, &mut visit);
}

fn walk<F>(cursor: &mut TreeCursor, source: &str, target: &str, visit: &mut F)
where
    F: FnMut(Node<'_>, usize, usize),
{
    let node = cursor.node();
    if matches!(node.kind(), "import_statement" | "export_statement")
        && let Some(specifier) = node.child_by_field_name("source")
        && specifier.kind() == "string"
        && let Some((inner_start, inner_end, value)) = string_inner_bytes(specifier, source)
        && value == target
    {
        visit(node, inner_start, inner_end);
    }

    if cursor.goto_first_child() {
        loop {
            walk(cursor, source, target, visit);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// Given a `string` node (with surrounding quotes), return the byte
/// range of its inner content and the string value.
fn string_inner_bytes<'a>(string_node: Node<'a>, source: &'a str) -> Option<(usize, usize, &'a str)> {
    // tree-sitter-javascript wraps the inner text in either
    // `string_fragment` children or matches the literal directly. The
    // safest approach: look for a `string_fragment` named child.
    for i in 0..string_node.named_child_count() {
        let Some(child) = string_node.named_child(i) else {
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
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let Some(tree) = parse_js(source_code) else {
        return source_code.to_string();
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for_each_matching_specifier(&tree, source_code, from, |_stmt, start, end| {
        edits.push((start, end, to.to_string()));
    });
    if edits.is_empty() {
        return source_code.to_string();
    }
    edits.sort_by_key(|e| e.0);
    let mut out = source_code.to_string();
    for (start, end, replacement) in edits.iter().rev() {
        out.replace_range(*start..*end, replacement);
    }
    out
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
}

pub fn delete(target: &str, source_code: &str) -> String {
    let Some(tree) = parse_js(source_code) else {
        return source_code.to_string();
    };
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for_each_matching_specifier(&tree, source_code, target, |stmt, _, _| {
        let start = stmt.start_byte();
        let end = stmt.end_byte();
        let line_start = source_code[..start].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let line_end = source_code[end..]
            .find('\n')
            .map(|p| end + p + 1)
            .unwrap_or(source_code.len());
        spans.push((line_start, line_end));
    });
    if spans.is_empty() {
        return source_code.to_string();
    }
    spans.sort_by_key(|s| s.0);
    spans.dedup();
    let mut out = source_code.to_string();
    for (start, end) in spans.iter().rev() {
        out.replace_range(*start..*end, "");
    }
    out
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
