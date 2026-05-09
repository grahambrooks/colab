//! Operations on Go `import` declarations: rename, delete, ensure.
//!
//! All three traverse the parse tree to locate `import_spec` nodes
//! whose `path` field equals the target string exactly. Edits are
//! applied in reverse byte order so earlier offsets remain valid.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Parser, Tree, TreeCursor};

fn parse_go(source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .expect("failed to load tree-sitter Go grammar");
    parser.parse(source, None)
}

/// Iterate every `import_spec` node whose `path` field equals
/// `target` exactly, calling `visit` with the spec node.
fn for_each_matching_import<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
where
    F: FnMut(Node<'_>),
{
    let mut cursor = tree.walk();
    walk(&mut cursor, source, target, &mut visit);
}

fn walk<F>(cursor: &mut TreeCursor, source: &str, target: &str, visit: &mut F)
where
    F: FnMut(Node<'_>),
{
    let node = cursor.node();
    if node.is_named()
        && node.kind() == "import_spec"
        && let Some(path_node) = node.child_by_field_name("path")
        && let Ok(path_text) = path_node.utf8_text(source.as_bytes())
        && path_text.trim_matches('"') == target
    {
        visit(node);
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

/// `Operation` that renames Go import paths whose value contains
/// `from`, substituting `to`.
#[derive(Debug)]
pub struct ImportRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for ImportRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "go::import \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for ImportRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("go")
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }
}

/// Replace every Go import whose path equals `from` with `to`.
///
/// Returns `source_code` unchanged if no imports match.
pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let Some(tree) = parse_go(source_code) else {
        return source_code.to_string();
    };

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for_each_matching_import(&tree, source_code, from, |node| {
        if let Some(path_node) = node.child_by_field_name("path") {
            edits.push((
                path_node.start_byte(),
                path_node.end_byte(),
                format!("\"{}\"", to),
            ));
        }
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

/// `Operation` that removes Go imports with a matching path.
#[derive(Debug)]
pub struct ImportDelete {
    pub target: String,
}

impl fmt::Display for ImportDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "go::import \"{}\" -> delete", self.target)
    }
}

impl Operation for ImportDelete {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("go")
    }

    fn apply(&self, source_code: &str) -> String {
        delete(&self.target, source_code)
    }
}

/// Remove every Go import whose path equals `target`. Erases the line
/// the `import_spec` lives on so a single import inside an
/// `import (…)` block disappears cleanly.
pub fn delete(target: &str, source_code: &str) -> String {
    let Some(tree) = parse_go(source_code) else {
        return source_code.to_string();
    };

    let mut spans: Vec<(usize, usize)> = Vec::new();
    for_each_matching_import(&tree, source_code, target, |node| {
        let start = node.start_byte();
        let end = node.end_byte();
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

/// `Operation` that idempotently adds a Go import if it is missing.
#[derive(Debug)]
pub struct ImportEnsure {
    pub target: String,
}

impl fmt::Display for ImportEnsure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "go::import \"{}\" -> ensure", self.target)
    }
}

impl Operation for ImportEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("go")
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(&self.target, source_code)
    }
}

/// Idempotently add `target` as a Go import if no `import_spec`
/// already references it. The new import is added as a standalone
/// `import "<target>"` line just after the `package` clause; users
/// who prefer block-form can run `gofmt` afterwards.
pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = parse_go(source_code) else {
        return source_code.to_string();
    };

    let mut already_present = false;
    for_each_matching_import(&tree, source_code, target, |_| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    // Insert immediately after the package clause's terminating
    // newline. If there is no package clause (uncommon — partial Go
    // file fragment), fall back to prepending.
    let root = tree.root_node();
    let mut insert_at: Option<usize> = None;
    for i in 0..root.named_child_count() {
        if let Some(child) = root.named_child(i)
            && child.kind() == "package_clause"
        {
            let end = child.end_byte();
            // Skip the newline after the package clause if any.
            insert_at = Some(
                source_code[end..]
                    .find('\n')
                    .map(|p| end + p + 1)
                    .unwrap_or(end),
            );
            break;
        }
    }

    let insertion = format!("\nimport \"{}\"\n", target);
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

    // ---------- rename ---------------------------------------------------

    #[test]
    fn renames_a_single_import() {
        let go_code = "package main\n\nimport (\n\t\"fmt\"\n\t\"some.module\"\n)\n";
        let out = rename("some.module", "new.module", go_code);
        assert!(out.contains("\"new.module\""));
        assert!(!out.contains("\"some.module\""));
    }

    #[test]
    fn does_not_match_path_substrings() {
        let go_code = "package main\n\nimport \"yet.another.module\"\n";
        assert_eq!(rename("another.module", "wrong", go_code), go_code);
    }

    #[test]
    fn rename_is_idempotent_on_its_own_output() {
        let go_code = "package main\n\nimport (\n\t\"some.module\"\n\t\"another.module\"\n)\n";
        let once = rename("some.module", "another.module", go_code);
        let twice = rename("some.module", "another.module", &once);
        assert_eq!(once, twice);
    }

    // ---------- delete ---------------------------------------------------

    #[test]
    fn deletes_a_single_block_import() {
        let go_code =
            "package main\n\nimport (\n\t\"fmt\"\n\t\"some.module\"\n\t\"other.module\"\n)\n";
        let out = delete("some.module", go_code);
        assert!(!out.contains("some.module"));
        assert!(out.contains("\"fmt\""));
        assert!(out.contains("\"other.module\""));
    }

    #[test]
    fn delete_is_idempotent() {
        let go_code = "package main\n\nimport \"some.module\"\n";
        let once = delete("some.module", go_code);
        let twice = delete("some.module", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn delete_no_match_returns_input_unchanged() {
        let go_code = "package main\n";
        assert_eq!(delete("missing", go_code), go_code);
    }

    // ---------- ensure ---------------------------------------------------

    #[test]
    fn ensure_adds_missing_import_after_package_clause() {
        let go_code = "package main\n\nfunc main() {}\n";
        let out = ensure("fmt", go_code);
        assert!(out.contains("import \"fmt\""), "got: {out}");
        // Inserted between package and func main.
        let pkg_pos = out.find("package main").unwrap();
        let import_pos = out.find("import \"fmt\"").unwrap();
        let func_pos = out.find("func main").unwrap();
        assert!(pkg_pos < import_pos && import_pos < func_pos, "got: {out}");
    }

    #[test]
    fn ensure_is_noop_when_already_imported() {
        let go_code = "package main\n\nimport \"fmt\"\n\nfunc main() {}\n";
        assert_eq!(ensure("fmt", go_code), go_code);
    }

    #[test]
    fn ensure_is_noop_when_already_in_block_form() {
        let go_code = "package main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n";
        assert_eq!(ensure("fmt", go_code), go_code);
    }

    #[test]
    fn ensure_is_idempotent() {
        let go_code = "package main\n\nfunc main() {}\n";
        let once = ensure("fmt", go_code);
        let twice = ensure("fmt", &once);
        assert_eq!(once, twice);
    }
}
