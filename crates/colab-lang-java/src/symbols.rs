//! Symbol rename for Java.
//!
//! Rewrites `identifier` and `type_identifier` nodes whose text
//! equals the target. Syntactic, not semantic — shadowing is not
//! analysed.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::TreeCursor;

const RENAME_KINDS: &[&str] = &["identifier", "type_identifier"];

#[derive(Debug)]
pub struct SymbolRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for SymbolRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "java::symbol \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for SymbolRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("java")
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
    fn renames_class_and_constructor_call() {
        let src = "package demo;\n\npublic class OldName {\n    public OldName() {}\n}\n\nclass User { OldName x = new OldName(); }\n";
        let out = rename("OldName", "NewName", src);
        assert!(out.contains("public class NewName"));
        assert!(out.contains("public NewName()"));
        assert!(out.contains("NewName x = new NewName();"));
    }

    #[test]
    fn renames_method_and_calls() {
        let src = "class A { void oldM() {} void caller() { oldM(); } }\n";
        let out = rename("oldM", "newM", src);
        assert_eq!(
            out,
            "class A { void newM() {} void caller() { newM(); } }\n"
        );
    }

    #[test]
    fn does_not_rename_string_literal() {
        let src = "class A { String s = \"OldName\"; }\n";
        assert_eq!(rename("OldName", "NewName", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "class A { void oldM() {} void caller() { oldM(); } }\n";
        let once = rename("oldM", "newM", src);
        let twice = rename("oldM", "newM", &once);
        assert_eq!(once, twice);
    }
}
