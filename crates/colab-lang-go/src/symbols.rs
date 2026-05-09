//! Symbol rename for Go.
//!
//! Visits `identifier`, `type_identifier`, and `field_identifier`
//! nodes whose text equals the target name and rewrites them.
//! Package identifiers (`package foo`, `import "p"`) and label
//! names are deliberately excluded so renaming a function `Foo`
//! never touches a package literal of the same name.
//!
//! This is a *syntactic* rename: scope and shadowing are not
//! analysed. If a local variable in some function happens to share a
//! name with a top-level type, both are renamed. Verify with
//! `--format diff` before applying.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::TreeCursor;

const RENAME_KINDS: &[&str] = &["identifier", "type_identifier", "field_identifier"];

#[derive(Debug)]
pub struct SymbolRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for SymbolRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "go::symbol \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for SymbolRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("go")
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
        let src = "package main\n\nfunc Foo() {}\n\nfunc main() { Foo() }\n";
        let out = rename("Foo", "Bar", src);
        assert_eq!(
            out,
            "package main\n\nfunc Bar() {}\n\nfunc main() { Bar() }\n"
        );
    }

    #[test]
    fn renames_struct_type_and_method_receiver() {
        let src = "package main\n\ntype Old struct { X int }\nfunc (o *Old) M() {}\n";
        let out = rename("Old", "New", src);
        assert!(out.contains("type New struct"));
        assert!(out.contains("(o *New)"));
    }

    #[test]
    fn does_not_rename_package_identifier_or_string_literal() {
        // `Foo` appears inside a string and as an import path — must
        // not be renamed.
        let src = "package main\n\nimport \"Foo\"\n\nvar s = \"Foo\"\nfunc main() { _ = s }\n";
        let out = rename("Foo", "Bar", src);
        assert_eq!(out, src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "package main\n\ntype Foo int\nfunc main() { var x Foo = 1; _ = x }\n";
        let once = rename("Foo", "Bar", src);
        let twice = rename("Foo", "Bar", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn returns_input_unchanged_when_symbol_absent() {
        let src = "package main\n\nfunc main() {}\n";
        assert_eq!(rename("Foo", "Bar", src), src);
    }

    #[test]
    fn symbol_rename_operation_is_relevant_for_go_files() {
        let op = SymbolRename {
            from: "a".into(),
            to: "b".into(),
        };
        assert!(op.is_file_relevant(Path::new("foo.go")));
        assert!(!op.is_file_relevant(Path::new("foo.rs")));
    }
}
