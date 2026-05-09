//! Python `import` operations.
//!
//! Tree-sitter-python exposes:
//! - `import_statement` with `name` field(s) → `dotted_name` or
//!   `aliased_import` (which itself has a `name: dotted_name` field).
//! - `import_from_statement` with `module_name` field → `dotted_name`
//!   or `relative_import` (we ignore relative imports for matching).
//!
//! We match the leading dotted-path segments of the module name.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Parser, Tree, TreeCursor};

fn parse_python(source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("failed to load tree-sitter Python grammar");
    parser.parse(source, None)
}

/// If `text` segment-prefix matches `from`, return the matched length.
fn match_path_prefix(text: &str, from: &str) -> Option<usize> {
    if !text.starts_with(from) {
        return None;
    }
    let after = &text[from.len()..];
    if after.is_empty() || after.starts_with('.') {
        Some(from.len())
    } else {
        None
    }
}

/// Visit every `dotted_name` node that lives inside an `import_*`
/// statement and whose text segment-prefix-matches `target`. The
/// visitor receives the enclosing import statement node, the
/// dotted_name node, and the matched prefix length.
fn for_each_matching_import<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
where
    F: FnMut(Node<'_>, Node<'_>, usize),
{
    let mut cursor = tree.walk();
    walk(&mut cursor, source, target, &mut visit);
}

fn walk<F>(cursor: &mut TreeCursor, source: &str, target: &str, visit: &mut F)
where
    F: FnMut(Node<'_>, Node<'_>, usize),
{
    let node = cursor.node();
    match node.kind() {
        "import_statement" => visit_import_statement(node, source, target, visit),
        "import_from_statement" => visit_import_from(node, source, target, visit),
        _ => {}
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

fn visit_import_statement<'a, F>(node: Node<'a>, source: &'a str, target: &str, visit: &mut F)
where
    F: FnMut(Node<'a>, Node<'a>, usize),
{
    // `import_statement` has multiple `name` children.
    for i in 0..node.named_child_count() {
        let Some(child) = node.named_child(i) else {
            break;
        };
        let dotted = match child.kind() {
            "dotted_name" => child,
            "aliased_import" => match child.child_by_field_name("name") {
                Some(n) if n.kind() == "dotted_name" => n,
                _ => continue,
            },
            _ => continue,
        };
        let Ok(text) = dotted.utf8_text(source.as_bytes()) else {
            continue;
        };
        if let Some(len) = match_path_prefix(text, target) {
            visit(node, dotted, len);
        }
    }
}

fn visit_import_from<'a, F>(node: Node<'a>, source: &'a str, target: &str, visit: &mut F)
where
    F: FnMut(Node<'a>, Node<'a>, usize),
{
    let Some(module) = node.child_by_field_name("module_name") else {
        return;
    };
    if module.kind() != "dotted_name" {
        return;
    }
    let Ok(text) = module.utf8_text(source.as_bytes()) else {
        return;
    };
    if let Some(len) = match_path_prefix(text, target) {
        visit(node, module, len);
    }
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ImportRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for ImportRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "python::import \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for ImportRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("py")
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let Some(tree) = parse_python(source_code) else {
        return source_code.to_string();
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for_each_matching_import(&tree, source_code, from, |_stmt, dotted, prefix_len| {
        let start = dotted.start_byte();
        edits.push((start, start + prefix_len, to.to_string()));
    });
    apply_edits(source_code, edits)
}

fn apply_edits(source: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    if edits.is_empty() {
        return source.to_string();
    }
    edits.sort_by_key(|e| e.0);
    let mut out = source.to_string();
    for (start, end, replacement) in edits.iter().rev() {
        out.replace_range(*start..*end, replacement);
    }
    out
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ImportDelete {
    pub target: String,
}

impl fmt::Display for ImportDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "python::import \"{}\" -> delete", self.target)
    }
}

impl Operation for ImportDelete {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("py")
    }

    fn apply(&self, source_code: &str) -> String {
        delete(&self.target, source_code)
    }
}

pub fn delete(target: &str, source_code: &str) -> String {
    let Some(tree) = parse_python(source_code) else {
        return source_code.to_string();
    };
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for_each_matching_import(&tree, source_code, target, |stmt, _, _| {
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
    // Dedup overlapping spans (`import a, b` could fire twice for the
    // same statement).
    spans.sort_by_key(|s| s.0);
    spans.dedup();
    let mut out = source_code.to_string();
    for (start, end) in spans.iter().rev() {
        out.replace_range(*start..*end, "");
    }
    out
}

// ---------------------------------------------------------------------------
// Ensure
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ImportEnsure {
    pub target: String,
}

impl fmt::Display for ImportEnsure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "python::import \"{}\" -> ensure", self.target)
    }
}

impl Operation for ImportEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("py")
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(&self.target, source_code)
    }
}

pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = parse_python(source_code) else {
        return source_code.to_string();
    };
    let mut already_present = false;
    for_each_matching_import(&tree, source_code, target, |_, _, _| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    // Insert at file top, after any leading shebang, encoding line,
    // module docstring, or `from __future__` imports.
    let insert_at = leading_header_end(&tree, source_code);
    let insertion = format!("import {}\n", target);
    let mut out = String::with_capacity(source_code.len() + insertion.len());
    out.push_str(&source_code[..insert_at]);
    out.push_str(&insertion);
    out.push_str(&source_code[insert_at..]);
    out
}

/// Skip over leading file-prologue nodes (module docstring,
/// `from __future__` imports). Returns the byte offset to insert at.
fn leading_header_end(tree: &Tree, source: &str) -> usize {
    let root = tree.root_node();
    let mut last_end = 0usize;
    for i in 0..root.named_child_count() {
        let Some(child) = root.named_child(i) else {
            break;
        };
        let is_header = match child.kind() {
            "expression_statement" => {
                // Module docstring: a single string literal as the
                // first statement.
                child
                    .named_child(0)
                    .map(|c| c.kind() == "string")
                    .unwrap_or(false)
            }
            "future_import_statement" => true,
            "import_from_statement" => child
                .child_by_field_name("module_name")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(|t| t == "__future__")
                .unwrap_or(false),
            _ => false,
        };
        if is_header {
            let end = child.end_byte();
            last_end = source[end..].find('\n').map(|p| end + p + 1).unwrap_or(end);
        } else {
            break;
        }
    }
    last_end
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- rename ---------------------------------------------------

    #[test]
    fn renames_import_statement() {
        let src = "import old.mod\nimport other\n";
        let out = rename("old.mod", "new.mod", src);
        assert!(out.contains("import new.mod"));
        assert!(out.contains("import other"));
    }

    #[test]
    fn renames_aliased_import() {
        let src = "import old.mod as m\n";
        let out = rename("old.mod", "new.mod", src);
        assert_eq!(out, "import new.mod as m\n");
    }

    #[test]
    fn renames_from_import() {
        let src = "from old.mod import foo\n";
        let out = rename("old.mod", "new.mod", src);
        assert_eq!(out, "from new.mod import foo\n");
    }

    #[test]
    fn renames_prefix_only() {
        let src = "import old.mod.deep\n";
        let out = rename("old", "new", src);
        assert_eq!(out, "import new.mod.deep\n");
    }

    #[test]
    fn does_not_match_substrings() {
        let src = "import oldsibling\n";
        assert_eq!(rename("old", "WRONG", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "import old.mod\nfrom old.mod import x\n";
        let once = rename("old", "new", src);
        let twice = rename("old", "new", &once);
        assert_eq!(once, twice);
    }

    // ---------- delete ---------------------------------------------------

    #[test]
    fn deletes_import_statement() {
        let src = "import os\nimport old.mod\nimport sys\n";
        let out = delete("old.mod", src);
        assert!(!out.contains("old.mod"));
        assert!(out.contains("import os"));
        assert!(out.contains("import sys"));
    }

    #[test]
    fn deletes_from_import() {
        let src = "from old.mod import foo\nimport sys\n";
        let out = delete("old.mod", src);
        assert!(!out.contains("old.mod"));
        assert!(out.contains("import sys"));
    }

    #[test]
    fn delete_is_idempotent() {
        let src = "import old.mod\n";
        let once = delete("old.mod", src);
        let twice = delete("old.mod", &once);
        assert_eq!(once, twice);
    }

    // ---------- ensure ---------------------------------------------------

    #[test]
    fn ensure_inserts_when_missing() {
        let src = "x = 1\n";
        let out = ensure("os", src);
        assert!(out.starts_with("import os\n"), "got: {out}");
    }

    #[test]
    fn ensure_is_noop_when_exact_present() {
        let src = "import os\nx = 1\n";
        assert_eq!(ensure("os", src), src);
    }

    #[test]
    fn ensure_is_noop_when_prefix_covered() {
        // ensure "x.y" is satisfied when `import x.y.z` exists.
        let src = "import x.y.z\n";
        assert_eq!(ensure("x.y", src), src);
    }

    #[test]
    fn ensure_inserts_after_module_docstring() {
        let src = "\"\"\"Module doc.\"\"\"\nx = 1\n";
        let out = ensure("os", src);
        assert!(
            out.starts_with("\"\"\"Module doc.\"\"\"\nimport os\n"),
            "got: {out}"
        );
    }

    #[test]
    fn ensure_inserts_after_future_imports() {
        let src = "from __future__ import annotations\n\nx = 1\n";
        let out = ensure("os", src);
        let future_pos = out.find("from __future__").unwrap();
        let os_pos = out.find("import os").unwrap();
        let x_pos = out.find("x = 1").unwrap();
        assert!(future_pos < os_pos && os_pos < x_pos, "got: {out}");
    }

    #[test]
    fn ensure_is_idempotent() {
        let src = "x = 1\n";
        let once = ensure("os", src);
        let twice = ensure("os", &once);
        assert_eq!(once, twice);
    }
}
