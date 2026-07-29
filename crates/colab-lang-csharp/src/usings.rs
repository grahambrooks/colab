//! Operations on C# `using` directives: rename, delete, ensure.
//!
//! A `using_directive` covers three forms, all matched on the dotted
//! name being imported:
//!
//! ```csharp
//! using System.Text;              // plain
//! using static Foo.Bar;           // static import
//! using Alias = Foo.Bar;          // alias
//! ```
//!
//! For the alias form the target is the *right-hand side* (`Foo.Bar`),
//! since that is the thing being imported — renaming an alias is a
//! `csharp::symbol` job.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Tree, TreeCursor};

/// The dotted-name node a `using_directive` imports, plus its text.
///
/// Takes the *last* qualified name/identifier child so the alias form
/// (`using Alias = Foo.Bar;`) yields `Foo.Bar` rather than `Alias`.
fn using_name<'a>(node: Node<'a>, source: &'a str) -> Option<(Node<'a>, &'a str)> {
    if node.kind() != "using_directive" {
        return None;
    }
    let mut found = None;
    for i in 0..node.named_child_count() {
        let Some(child) = node.named_child(i as u32) else {
            break;
        };
        if matches!(child.kind(), "qualified_name" | "identifier")
            && let Ok(text) = child.utf8_text(source.as_bytes())
        {
            found = Some((child, text));
        }
    }
    found
}

fn for_each_matching_using<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
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
    if let Some((name_node, text)) = using_name(node, source)
        && text == target
    {
        visit(node, name_node);
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
pub struct UsingRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for UsingRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "csharp::using \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for UsingRename {
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
    for_each_matching_using(&tree, source_code, from, |_directive, name_node| {
        edits.push((name_node.start_byte(), name_node.end_byte(), to.to_string()));
    });
    apply_edits(source_code, edits)
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct UsingDelete {
    pub target: String,
}

impl fmt::Display for UsingDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "csharp::using \"{}\" -> delete", self.target)
    }
}

impl Operation for UsingDelete {
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
    for_each_matching_using(&tree, source_code, target, |directive, _name| {
        let start = directive.start_byte();
        let end = directive.end_byte();
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
pub struct UsingEnsure {
    pub target: String,
}

impl fmt::Display for UsingEnsure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "csharp::using \"{}\" -> ensure", self.target)
    }
}

impl Operation for UsingEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(&self.target, source_code)
    }

    // No prefilter: `ensure` acts precisely when the target is absent.
}

/// Insert `using <target>;` after the last existing using directive, or
/// at the top of the file when there is none.
pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut already_present = false;
    for_each_matching_using(&tree, source_code, target, |_, _| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    let mut insert_at = 0usize;
    let root = tree.root_node();
    for i in 0..root.named_child_count() {
        let Some(child) = root.named_child(i as u32) else {
            break;
        };
        if child.kind() == "using_directive" {
            let end = child.end_byte();
            insert_at = source_code[end..]
                .find('\n')
                .map(|p| end + p + 1)
                .unwrap_or(source_code.len());
        }
    }

    let directive = format!("using {};\n", target);
    let mut out = String::with_capacity(source_code.len() + directive.len());
    out.push_str(&source_code[..insert_at]);
    out.push_str(&directive);
    out.push_str(&source_code[insert_at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str =
        "using System;\nusing System.Text;\n\nnamespace Demo;\n\nclass C { }\n";

    #[test]
    fn renames_a_using_directive() {
        let out = rename("System.Text", "System.Text.Json", SRC);
        assert!(out.contains("using System.Text.Json;"), "got: {out}");
        assert!(out.contains("using System;"), "other usings untouched");
    }

    #[test]
    fn matches_the_static_form() {
        let src = "using static Foo.Bar;\n";
        let out = rename("Foo.Bar", "Foo.Baz", src);
        assert_eq!(out, "using static Foo.Baz;\n");
    }

    #[test]
    fn alias_form_targets_the_right_hand_side() {
        let src = "using Alias = Foo.Bar;\n";
        let out = rename("Foo.Bar", "Foo.Baz", src);
        assert_eq!(out, "using Alias = Foo.Baz;\n");
        // And the alias itself is not the target.
        assert_eq!(rename("Alias", "Other", src), src);
    }

    #[test]
    fn rename_does_not_match_a_substring() {
        // `System` must not partial-match `System.Text`.
        let src = "using System.Text;\n";
        let out = rename("System", "WRONG", src);
        assert_eq!(out, src);
    }

    #[test]
    fn rename_is_idempotent() {
        let once = rename("System.Text", "System.Text.Json", SRC);
        let twice = rename("System.Text", "System.Text.Json", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn deletes_the_matched_line() {
        let out = delete("System.Text", SRC);
        assert!(!out.contains("System.Text"));
        assert!(out.contains("using System;"));
        assert!(out.contains("class C"));
    }

    #[test]
    fn delete_is_idempotent() {
        let once = delete("System.Text", SRC);
        let twice = delete("System.Text", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn ensure_adds_after_the_existing_usings() {
        let out = ensure("System.Linq", SRC);
        assert!(out.contains("using System.Linq;"), "got: {out}");
        let last_existing = out.find("using System.Text;").unwrap();
        let added = out.find("using System.Linq;").unwrap();
        let ns = out.find("namespace Demo").unwrap();
        assert!(last_existing < added && added < ns, "got: {out}");
    }

    #[test]
    fn ensure_is_a_noop_when_present() {
        assert_eq!(ensure("System.Text", SRC), SRC);
    }

    #[test]
    fn ensure_prepends_when_there_are_no_usings() {
        let src = "class C { }\n";
        let out = ensure("System", src);
        assert!(out.starts_with("using System;\n"), "got: {out}");
    }

    #[test]
    fn ensure_is_idempotent() {
        let once = ensure("System.Linq", SRC);
        let twice = ensure("System.Linq", &once);
        assert_eq!(once, twice);
    }
}
