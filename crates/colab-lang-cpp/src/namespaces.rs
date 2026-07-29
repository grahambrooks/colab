//! Rename a C++ `namespace` declaration.
//!
//! `namespace_definition` carries a `name` field that is either a
//! `namespace_identifier` (`namespace foo {`) or a
//! `nested_namespace_specifier` (`namespace foo::bar {`). An anonymous
//! namespace has no `name` field at all and never matches.
//!
//! Matching is exact on the declared name, so `foo` matches
//! `namespace foo` but not `namespace foo::bar` — target the nested form
//! verbatim (`"foo::bar"`) when that is what you mean.
//!
//! This rewrites the *declaration* only. Qualified uses of the namespace
//! elsewhere in the file (`foo::thing`) are `namespace_identifier` nodes
//! that `cpp::symbol` covers, so a full rename is usually two rules.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::TreeCursor;

#[derive(Debug)]
pub struct NamespaceRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for NamespaceRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cpp::namespace \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for NamespaceRename {
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
    if node.kind() == "namespace_definition"
        && let Some(name) = node.child_by_field_name("name")
        && let Ok(text) = name.utf8_text(source.as_bytes())
        && text == from
    {
        out.push((name.start_byte(), name.end_byte()));
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
    fn renames_a_simple_namespace() {
        let src = "namespace old_ns {\nvoid f() {}\n}\n";
        let out = rename("old_ns", "new_ns", src);
        assert!(out.contains("namespace new_ns {"), "got: {out}");
    }

    #[test]
    fn renames_a_nested_namespace_specifier_verbatim() {
        let src = "namespace a::b {\n}\n";
        let out = rename("a::b", "c::d", src);
        assert!(out.contains("namespace c::d {"), "got: {out}");
    }

    #[test]
    fn a_simple_target_does_not_match_a_nested_declaration() {
        let src = "namespace a::b {\n}\n";
        assert_eq!(rename("a", "WRONG", src), src);
    }

    #[test]
    fn anonymous_namespaces_never_match() {
        let src = "namespace {\nvoid f() {}\n}\n";
        assert_eq!(rename("old_ns", "new_ns", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "namespace old_ns {\n}\n";
        let once = rename("old_ns", "new_ns", src);
        let twice = rename("old_ns", "new_ns", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "namespace other {\n}\n";
        assert_eq!(rename("old_ns", "new_ns", src), src);
    }
}
