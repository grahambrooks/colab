//! Operations on PHP `use` declarations: rename, delete, ensure.
//!
//! A `namespace_use_declaration` holds one or more
//! `namespace_use_clause`s, each naming a `qualified_name` (`App\Old\Thing`)
//! or a bare `name` (`use Thing;`). The `use function` and `use const`
//! forms have the same shape and are matched the same way.
//!
//! **Grouped imports are not matched.** `use App\Sub\{A, B};` parses with
//! the group members as bare clause names under a shared prefix, so a
//! target like `App\Sub\A` has no single node to match. Expand the group
//! before running a codemod over it — silently rewriting half of one
//! would be worse than not matching.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Tree, TreeCursor};

/// The name node of a `namespace_use_clause`, plus its text.
fn clause_name<'a>(node: Node<'a>, source: &'a str) -> Option<(Node<'a>, &'a str)> {
    if node.kind() != "namespace_use_clause" {
        return None;
    }
    for i in 0..node.named_child_count() {
        let Some(child) = node.named_child(i as u32) else {
            break;
        };
        if matches!(child.kind(), "qualified_name" | "name")
            && let Ok(text) = child.utf8_text(source.as_bytes())
        {
            return Some((child, text));
        }
    }
    None
}

/// Walk to each `namespace_use_clause` whose name equals `target`,
/// yielding the enclosing declaration and the name node.
///
/// Clauses inside a `namespace_use_group` are skipped — see module docs.
fn for_each_matching_use<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
where
    F: FnMut(Node<'_>, Node<'_>),
{
    let mut cursor = tree.walk();
    walk(&mut cursor, source, target, &mut visit);
}

fn walk<F>(cursor: &mut TreeCursor, source: &str, target: &str, visit: &mut F)
where
    F: FnMut(Node<'_>, Node<'_>),
{
    let node = cursor.node();
    if node.kind() == "namespace_use_declaration" {
        // A declaration containing a group is left alone entirely.
        let has_group = (0..node.named_child_count()).any(|i| {
            node.named_child(i as u32)
                .map(|c| c.kind() == "namespace_use_group")
                .unwrap_or(false)
        });
        if !has_group {
            for i in 0..node.named_child_count() {
                let Some(clause) = node.named_child(i as u32) else {
                    break;
                };
                if let Some((name_node, text)) = clause_name(clause, source)
                    && text == target
                {
                    visit(node, name_node);
                }
            }
        }
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
// Rename
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct UseRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for UseRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "php::use \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for UseRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
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
    for_each_matching_use(&tree, source_code, from, |_decl, name_node| {
        edits.push((name_node.start_byte(), name_node.end_byte(), to.to_string()));
    });
    apply_edits(source_code, edits)
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct UseDelete {
    pub target: String,
}

impl fmt::Display for UseDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "php::use \"{}\" -> delete", self.target)
    }
}

impl Operation for UseDelete {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
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
    for_each_matching_use(&tree, source_code, target, |decl, _name| {
        let start = decl.start_byte();
        let end = decl.end_byte();
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

// ---------------------------------------------------------------------------
// Ensure
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct UseEnsure {
    pub target: String,
}

impl fmt::Display for UseEnsure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "php::use \"{}\" -> ensure", self.target)
    }
}

impl Operation for UseEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(&self.target, source_code)
    }

    // No prefilter: `ensure` acts precisely when the target is absent.
}

/// Insert `use <target>;` after the last existing `use`, else after the
/// `namespace` declaration, else after the opening PHP tag.
pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut already_present = false;
    for_each_matching_use(&tree, source_code, target, |_, _| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    // Anchor preference, strongest first: after the last existing `use`,
    // else after the `namespace` declaration, else after the opening tag.
    // These are tracked separately because the tag always comes first in
    // source order and would otherwise win.
    let root = tree.root_node();
    let (mut after_use, mut after_namespace, mut after_tag) = (None, None, None);
    for i in 0..root.named_child_count() {
        let Some(child) = root.named_child(i as u32) else {
            break;
        };
        let end = child.end_byte();
        let line_end = source_code[end..]
            .find('\n')
            .map(|p| end + p + 1)
            .unwrap_or(source_code.len());
        match child.kind() {
            "namespace_use_declaration" => after_use = Some(line_end),
            "namespace_definition" => after_namespace = after_namespace.or(Some(line_end)),
            "php_tag" => after_tag = after_tag.or(Some(line_end)),
            _ => {}
        }
    }

    let pos = after_use.or(after_namespace).or(after_tag).unwrap_or(0);
    let insertion = format!("use {};\n", target);
    let mut out = String::with_capacity(source_code.len() + insertion.len());
    out.push_str(&source_code[..pos]);
    out.push_str(&insertion);
    out.push_str(&source_code[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "<?php\nnamespace App;\n\nuse App\\Old\\Thing;\nuse App\\Other;\n\nclass C {}\n";

    #[test]
    fn renames_a_qualified_use() {
        let out = rename("App\\Old\\Thing", "App\\New\\Thing", SRC);
        assert!(out.contains("use App\\New\\Thing;"), "got: {out}");
        assert!(out.contains("use App\\Other;"), "other uses untouched");
    }

    #[test]
    fn matches_the_use_function_form() {
        let src = "<?php\nuse function App\\Old\\helper;\n";
        let out = rename("App\\Old\\helper", "App\\New\\helper", src);
        assert_eq!(out, "<?php\nuse function App\\New\\helper;\n");
    }

    #[test]
    fn rename_does_not_match_a_prefix() {
        assert_eq!(rename("App\\Old", "WRONG", SRC), SRC);
    }

    #[test]
    fn grouped_imports_are_left_alone() {
        // Matching half a group would silently corrupt it.
        let src = "<?php\nuse App\\Sub\\{A, B};\n";
        assert_eq!(rename("App\\Sub\\A", "App\\Sub\\C", src), src);
        assert_eq!(delete("App\\Sub\\A", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let once = rename("App\\Old\\Thing", "App\\New\\Thing", SRC);
        let twice = rename("App\\Old\\Thing", "App\\New\\Thing", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn deletes_the_matched_line() {
        let out = delete("App\\Old\\Thing", SRC);
        assert!(!out.contains("App\\Old\\Thing"));
        assert!(out.contains("use App\\Other;"));
        assert!(out.contains("class C"));
    }

    #[test]
    fn delete_is_idempotent() {
        let once = delete("App\\Old\\Thing", SRC);
        let twice = delete("App\\Old\\Thing", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn ensure_adds_after_the_existing_uses() {
        let out = ensure("App\\Extra", SRC);
        assert!(out.contains("use App\\Extra;"), "got: {out}");
        let last_existing = out.find("use App\\Other;").unwrap();
        let added = out.find("use App\\Extra;").unwrap();
        assert!(last_existing < added, "got: {out}");
    }

    #[test]
    fn ensure_falls_back_to_after_the_namespace() {
        let src = "<?php\nnamespace App;\n\nclass C {}\n";
        let out = ensure("App\\Thing", src);
        let ns = out.find("namespace App;").unwrap();
        let added = out.find("use App\\Thing;").unwrap();
        let cls = out.find("class C").unwrap();
        assert!(ns < added && added < cls, "got: {out}");
    }

    #[test]
    fn ensure_is_a_noop_when_present() {
        assert_eq!(ensure("App\\Old\\Thing", SRC), SRC);
    }

    #[test]
    fn ensure_is_idempotent() {
        let once = ensure("App\\Extra", SRC);
        let twice = ensure("App\\Extra", &once);
        assert_eq!(once, twice);
    }
}
