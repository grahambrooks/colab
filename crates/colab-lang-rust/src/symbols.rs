//! Symbol rename for Rust.
//!
//! Rewrites every `identifier`, `type_identifier`, `field_identifier`,
//! and `shorthand_field_identifier` node whose text equals the
//! target. Macro names, lifetimes, label names, and tokens that
//! merely look like identifiers (string contents, etc.) are
//! deliberately excluded.
//!
//! Syntactic, not semantic — shadowing and scope are not analysed.
//! Verify with `--format diff` before applying.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::TreeCursor;

const RENAME_KINDS: &[&str] = &[
    "identifier",
    "type_identifier",
    "field_identifier",
    "shorthand_field_identifier",
];

#[derive(Debug)]
pub struct SymbolRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for SymbolRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rust::symbol \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for SymbolRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("rs")
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
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
    if RENAME_KINDS.contains(&node.kind())
        && let Ok(text) = node.utf8_text(source.as_bytes())
        && text == from
    {
        out.push((node.start_byte(), node.end_byte()));
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
    fn renames_function_declaration_and_calls() {
        let src = "fn old_fn() {}\n\nfn main() { old_fn(); }\n";
        let out = rename("old_fn", "new_fn", src);
        assert_eq!(out, "fn new_fn() {}\n\nfn main() { new_fn(); }\n");
    }

    #[test]
    fn renames_struct_and_field_uses() {
        let src = "struct Old { x: i32 }\nfn main() { let _ = Old { x: 1 }; }\n";
        let out = rename("Old", "New", src);
        assert!(out.contains("struct New"));
        assert!(out.contains("New { x: 1 }"));
    }

    #[test]
    fn renames_field_identifier() {
        let src = "struct S { old_field: i32 }\nfn main() { let s = S { old_field: 1 }; let _ = s.old_field; }\n";
        let out = rename("old_field", "new_field", src);
        assert!(out.contains("new_field: i32"));
        assert!(out.contains("s.new_field"));
    }

    #[test]
    fn does_not_rename_string_literals() {
        let src = "fn main() { let _ = \"foo\"; }\n";
        assert_eq!(rename("foo", "WRONG", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "fn old_fn() {}\nfn main() { old_fn(); }\n";
        let once = rename("old_fn", "new_fn", src);
        let twice = rename("old_fn", "new_fn", &once);
        assert_eq!(once, twice);
    }
}
