//! Operations on Java `import` declarations.
//!
//! `import_declaration` nodes in tree-sitter-java contain either a
//! `scoped_identifier` (regular import) or a `scoped_identifier`
//! after a `static` keyword (`import static foo.Bar.baz;`). For
//! matching we extract the dotted name as a string.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Parser, Tree, TreeCursor};

fn parse_java(source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .expect("failed to load tree-sitter Java grammar");
    parser.parse(source, None)
}

fn import_name<'a>(node: Node<'a>, source: &'a str) -> Option<(Node<'a>, String)> {
    if !node.is_named() || node.kind() != "import_declaration" {
        return None;
    }
    // The identifier node is one of the import_declaration's named
    // children — it can be `scoped_identifier`, `identifier`, or
    // `asterisk` after `import com.x.*;`. We want a dotted name; for
    // `*` we return None so it never matches a literal target.
    for i in 0..node.named_child_count() {
        let child = node.named_child(i)?;
        match child.kind() {
            "scoped_identifier" | "identifier" => {
                if let Ok(text) = child.utf8_text(source.as_bytes()) {
                    return Some((child, text.to_string()));
                }
            }
            _ => {}
        }
    }
    None
}

fn for_each_matching_import<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
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
    if let Some((name_node, name)) = import_name(node, source)
        && name == target
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
        write!(f, "java::import \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for ImportRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("java")
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let Some(tree) = parse_java(source_code) else {
        return source_code.to_string();
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for_each_matching_import(&tree, source_code, from, |_decl, name_node| {
        edits.push((name_node.start_byte(), name_node.end_byte(), to.to_string()));
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
        write!(f, "java::import \"{}\" -> delete", self.target)
    }
}

impl Operation for ImportDelete {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("java")
    }

    fn apply(&self, source_code: &str) -> String {
        delete(&self.target, source_code)
    }
}

pub fn delete(target: &str, source_code: &str) -> String {
    let Some(tree) = parse_java(source_code) else {
        return source_code.to_string();
    };
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for_each_matching_import(&tree, source_code, target, |decl, _name| {
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
        write!(f, "java::import \"{}\" -> ensure", self.target)
    }
}

impl Operation for ImportEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("java")
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(&self.target, source_code)
    }
}

/// Insert `import <target>;` after the package declaration if no
/// existing import already names `<target>`.
pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = parse_java(source_code) else {
        return source_code.to_string();
    };
    let mut already_present = false;
    for_each_matching_import(&tree, source_code, target, |_, _| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    // Insert after the package_declaration if any; otherwise prepend.
    let root = tree.root_node();
    let mut insert_at: Option<usize> = None;
    for i in 0..root.named_child_count() {
        let Some(child) = root.named_child(i) else {
            break;
        };
        if child.kind() == "package_declaration" {
            let end = child.end_byte();
            insert_at = Some(
                source_code[end..]
                    .find('\n')
                    .map(|p| end + p + 1)
                    .unwrap_or(end),
            );
            break;
        }
    }

    let insertion = format!("\nimport {};\n", target);
    let pos = insert_at.unwrap_or(0);
    let mut out = String::with_capacity(source_code.len() + insertion.len());
    out.push_str(&source_code[..pos]);
    out.push_str(&insertion);
    out.push_str(&source_code[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str = "package demo;\n\nimport java.util.List;\nimport java.util.Map;\n\npublic class Hello {}\n";

    #[test]
    fn renames_a_named_import() {
        let out = rename("java.util.List", "java.util.ArrayList", HELLO);
        assert!(out.contains("import java.util.ArrayList;"));
        assert!(!out.contains("import java.util.List;"));
        assert!(out.contains("import java.util.Map;"));
    }

    #[test]
    fn rename_does_not_match_substring() {
        let src = "package demo;\nimport com.example.MyList;\n";
        // `java.util.List` must not partial-match `com.example.MyList`.
        assert_eq!(rename("java.util.List", "WRONG", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let once = rename("java.util.List", "java.util.ArrayList", HELLO);
        let twice = rename("java.util.List", "java.util.ArrayList", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn deletes_the_matched_import_line() {
        let out = delete("java.util.List", HELLO);
        assert!(!out.contains("java.util.List"));
        assert!(out.contains("import java.util.Map;"));
    }

    #[test]
    fn delete_no_match_returns_input() {
        let src = "package demo;\nimport java.util.Map;\n";
        assert_eq!(delete("java.util.List", src), src);
    }

    #[test]
    fn delete_is_idempotent() {
        let once = delete("java.util.List", HELLO);
        let twice = delete("java.util.List", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn ensure_adds_missing_import_after_package() {
        let src = "package demo;\n\npublic class Hello {}\n";
        let out = ensure("java.util.List", src);
        assert!(out.contains("import java.util.List;"), "got: {out}");
        let pkg = out.find("package demo").unwrap();
        let imp = out.find("import java.util.List").unwrap();
        let cls = out.find("public class").unwrap();
        assert!(pkg < imp && imp < cls, "got: {out}");
    }

    #[test]
    fn ensure_is_noop_when_already_imported() {
        assert_eq!(ensure("java.util.List", HELLO), HELLO);
    }

    #[test]
    fn ensure_is_idempotent() {
        let src = "package demo;\n\npublic class Hello {}\n";
        let once = ensure("java.util.List", src);
        let twice = ensure("java.util.List", &once);
        assert_eq!(once, twice);
    }
}
