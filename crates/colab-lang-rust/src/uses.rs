//! Operations on Rust `use` declarations: rename, delete, ensure.
//!
//! Path matching is segment-wise: `from = "tokio"` matches `tokio`,
//! `tokio::sync::Mutex`, `tokio as t`, but never `my_tokio` or
//! `foo::tokio`. Rename rewrites the matched prefix; delete erases
//! the whole `use` line; ensure inserts a new `use <target>;` if no
//! existing declaration starts with the same path.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Tree, TreeCursor};

/// Visit every `use_declaration` whose argument's leading path
/// segment-matches `target`. The visitor receives the use_declaration
/// node, the argument node, and the byte length of the matching
/// prefix inside the argument.
fn for_each_matching_use<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
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
    if node.is_named()
        && node.kind() == "use_declaration"
        && let Some(arg) = node.child_by_field_name("argument")
        && let Ok(arg_text) = arg.utf8_text(source.as_bytes())
        && let Some(prefix_len) = match_path_prefix(arg_text, target)
    {
        visit(node, arg, prefix_len);
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

/// If `text` begins with `from` followed by a path-segment boundary
/// (`::`, end of path, whitespace, or `;`), returns the byte length
/// of the matched prefix.
fn match_path_prefix(text: &str, from: &str) -> Option<usize> {
    if !text.starts_with(from) {
        return None;
    }
    let after = &text[from.len()..];
    if after.is_empty()
        || after.starts_with("::")
        || after.starts_with(' ')
        || after.starts_with('\t')
        || after.starts_with('\n')
        || after.starts_with(';')
    {
        Some(from.len())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

/// `Operation` that renames a leading path prefix in `use` declarations.
#[derive(Debug)]
pub struct UseRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for UseRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rust::use \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for UseRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("rs")
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }
}

/// Rewrite the source: replace any `use` whose path starts with
/// segments equal to `from` so the prefix becomes `to`.
pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for_each_matching_use(&tree, source_code, from, |_use_decl, arg, prefix_len| {
        let start = arg.start_byte();
        edits.push((start, start + prefix_len, to.to_string()));
    });

    apply_edits(source_code, edits)
}

fn apply_edits(source: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    if edits.is_empty() {
        return source.to_string();
    }
    edits.sort_by_key(|e| e.0);
    let mut rewritten = source.to_string();
    for (start, end, replacement) in edits.iter().rev() {
        rewritten.replace_range(*start..*end, replacement);
    }
    rewritten
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// `Operation` that removes `use` declarations whose leading path
/// segment-matches `target`.
#[derive(Debug)]
pub struct UseDelete {
    pub target: String,
}

impl fmt::Display for UseDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rust::use \"{}\" -> delete", self.target)
    }
}

impl Operation for UseDelete {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("rs")
    }

    fn apply(&self, source_code: &str) -> String {
        delete(&self.target, source_code)
    }
}

/// Remove every `use` declaration whose leading path equals `target`.
/// Erases the whole `use ...;` line including its trailing newline.
pub fn delete(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut spans: Vec<(usize, usize)> = Vec::new();
    for_each_matching_use(&tree, source_code, target, |use_decl, _, _| {
        let start = use_decl.start_byte();
        let end = use_decl.end_byte();
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
    let mut rewritten = source_code.to_string();
    for (start, end) in spans.iter().rev() {
        rewritten.replace_range(*start..*end, "");
    }
    rewritten
}

// ---------------------------------------------------------------------------
// Ensure
// ---------------------------------------------------------------------------

/// `Operation` that idempotently adds a `use <target>;` declaration
/// if no existing `use` already begins with the same path.
#[derive(Debug)]
pub struct UseEnsure {
    pub target: String,
}

impl fmt::Display for UseEnsure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rust::use \"{}\" -> ensure", self.target)
    }
}

impl Operation for UseEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("rs")
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(&self.target, source_code)
    }
}

/// Insert `use <target>;` at the top of `source_code` if no existing
/// `use` declaration leads with the same path. The new line goes
/// after any leading inner attributes (`#![...]`) so module-level
/// pragmas stay first.
pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut already_present = false;
    for_each_matching_use(&tree, source_code, target, |_, _, _| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    // Find the byte offset right after the last leading inner
    // attribute (`#![...]`) on the file's source order. If none,
    // insert at byte 0.
    let root = tree.root_node();
    let mut insert_at = 0usize;
    for i in 0..root.named_child_count() {
        let Some(child) = root.named_child(i) else {
            break;
        };
        if child.kind() == "inner_attribute_item" {
            insert_at = source_code[child.end_byte()..]
                .find('\n')
                .map(|p| child.end_byte() + p + 1)
                .unwrap_or(child.end_byte());
        } else {
            break;
        }
    }

    let insertion = format!("use {};\n", target);
    let mut out = String::with_capacity(source_code.len() + insertion.len());
    out.push_str(&source_code[..insert_at]);
    out.push_str(&insertion);
    out.push_str(&source_code[insert_at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- rename ---------------------------------------------------

    #[test]
    fn renames_simple_use_path() {
        let src = "use tokio::sync::Mutex;\n";
        let out = rename("tokio", "async_tokio", src);
        assert_eq!(out, "use async_tokio::sync::Mutex;\n");
    }

    #[test]
    fn does_not_substring_match_first_segment() {
        let src = "use my_tokio::sync;\n";
        assert_eq!(rename("tokio", "WRONG", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "use tokio::sync::Mutex;\nuse tokio::fs;\n";
        let once = rename("tokio", "async_tokio", src);
        let twice = rename("tokio", "async_tokio", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn use_rename_operation_is_relevant_for_rs_files_only() {
        let op = UseRename {
            from: "a".into(),
            to: "b".into(),
        };
        assert!(op.is_file_relevant(Path::new("foo.rs")));
        assert!(!op.is_file_relevant(Path::new("foo.go")));
        assert!(!op.is_file_relevant(Path::new("Cargo.toml")));
    }

    // ---------- delete ---------------------------------------------------

    #[test]
    fn deletes_matching_use_lines() {
        let src = "use tokio::sync::Mutex;\nuse std::fmt;\nuse tokio::fs;\n";
        let out = delete("tokio", src);
        assert!(!out.contains("tokio"));
        assert!(out.contains("use std::fmt;"));
    }

    #[test]
    fn delete_does_not_match_substrings() {
        let src = "use my_tokio::sync;\n";
        assert_eq!(delete("tokio", src), src);
    }

    #[test]
    fn delete_is_idempotent() {
        let src = "use tokio::sync::Mutex;\nuse std::fmt;\n";
        let once = delete("tokio", src);
        let twice = delete("tokio", &once);
        assert_eq!(once, twice);
    }

    // ---------- ensure ---------------------------------------------------

    #[test]
    fn ensure_inserts_when_missing() {
        let src = "fn main() {}\n";
        let out = ensure("std::fmt", src);
        assert!(out.starts_with("use std::fmt;\n"), "got: {out}");
    }

    #[test]
    fn ensure_is_noop_when_path_already_imported() {
        let src = "use std::fmt::Write;\n\nfn main() {}\n";
        // Existing `use` starts with "std::fmt", which segment-matches
        // ensuring "std::fmt".
        assert_eq!(ensure("std::fmt", src), src);
    }

    #[test]
    fn ensure_inserts_after_inner_attributes() {
        let src = "#![allow(dead_code)]\nfn main() {}\n";
        let out = ensure("std::fmt", src);
        assert!(
            out.starts_with("#![allow(dead_code)]\nuse std::fmt;\n"),
            "got: {out}"
        );
    }

    #[test]
    fn ensure_is_idempotent() {
        let src = "fn main() {}\n";
        let once = ensure("std::fmt", src);
        let twice = ensure("std::fmt", &once);
        assert_eq!(once, twice);
    }
}
