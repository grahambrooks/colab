//! Rename a Go `package` clause.
//!
//! `package_clause` holds a `package_identifier` with the package name.
//! Go package names are a single identifier — the import *path* is a
//! separate thing that `go::import` handles — so a package move is
//! usually this rule plus a `go::import` rule in the same script.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::TreeCursor;

#[derive(Debug)]
pub struct PackageRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for PackageRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "go::package \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for PackageRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("go")
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
    let mut edits: Vec<(usize, usize)> = Vec::new();
    let mut cursor = tree.walk();
    collect(&mut cursor, source_code, from, &mut edits);
    if edits.is_empty() {
        return source_code.to_string();
    }
    edits.sort_by_key(|e| e.0);
    let mut out = source_code.to_string();
    for (start, end) in edits.iter().rev() {
        out.replace_range(*start..*end, to);
    }
    out
}

fn collect(cursor: &mut TreeCursor, source: &str, from: &str, out: &mut Vec<(usize, usize)>) {
    let node = cursor.node();
    if node.kind() == "package_clause" {
        for i in 0..node.named_child_count() {
            let Some(child) = node.named_child(i as u32) else {
                break;
            };
            if child.kind() == "package_identifier"
                && let Ok(text) = child.utf8_text(source.as_bytes())
                && text == from
            {
                out.push((child.start_byte(), child.end_byte()));
            }
        }
    }

    if cursor.goto_first_child() {
        loop {
            collect(cursor, source, from, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_the_package_clause() {
        let src = "package oldpkg\n\nfunc f() {}\n";
        let out = rename("oldpkg", "newpkg", src);
        assert!(out.contains("package newpkg"), "got: {out}");
    }

    #[test]
    fn does_not_touch_a_matching_identifier_elsewhere() {
        // Only the package clause is in scope; the qualifier is not.
        let src = "package demo\n\nfunc f() { oldpkg.Run() }\n";
        assert_eq!(rename("oldpkg", "newpkg", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "package oldpkg\n";
        let once = rename("oldpkg", "newpkg", src);
        assert_eq!(rename("oldpkg", "newpkg", &once), once);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "package other\n";
        assert_eq!(rename("oldpkg", "newpkg", src), src);
    }
}
