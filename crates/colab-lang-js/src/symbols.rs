//! Symbol rename for JavaScript / TypeScript.
//!
//! Rewrites `identifier`, `property_identifier`, and
//! `shorthand_property_identifier` nodes whose text equals the
//! target. Syntactic, not semantic — JSX attribute names that share
//! a function name will also be touched. Verify with
//! `--format diff`.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Parser, TreeCursor};

use crate::imports::is_relevant;

const RENAME_KINDS: &[&str] = &[
    "identifier",
    "property_identifier",
    "shorthand_property_identifier",
];

#[derive(Debug)]
pub struct SymbolRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for SymbolRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "js::symbol \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for SymbolRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_javascript::LANGUAGE.into())
        .expect("failed to load tree-sitter JavaScript grammar");
    let Some(tree) = parser.parse(source_code, None) else {
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
    fn renames_function_decl_and_calls() {
        let src = "function oldFn() {}\noldFn();\n";
        let out = rename("oldFn", "newFn", src);
        assert_eq!(out, "function newFn() {}\nnewFn();\n");
    }

    #[test]
    fn renames_object_property_access() {
        let src = "const x = obj.oldProp;\n";
        let out = rename("oldProp", "newProp", src);
        assert_eq!(out, "const x = obj.newProp;\n");
    }

    #[test]
    fn does_not_rename_string_literal() {
        let src = "const x = \"oldFn\";\n";
        assert_eq!(rename("oldFn", "newFn", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "function oldFn() {}\noldFn();\n";
        let once = rename("oldFn", "newFn", src);
        let twice = rename("oldFn", "newFn", &once);
        assert_eq!(once, twice);
    }
}
