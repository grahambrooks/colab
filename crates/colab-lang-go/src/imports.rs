//! Operations on Go `import` declarations: rename, delete, ensure.
//!
//! All three traverse the parse tree to locate `import_spec` nodes
//! whose `path` field equals the target string exactly. Edits are
//! applied in reverse byte order so earlier offsets remain valid.

use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Tree};

/// Iterate every `import_spec` node whose `path` field equals
/// `target` exactly, calling `visit` with the spec node.
fn for_each_matching_import<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
where
    F: FnMut(Node<'_>),
{
    colab_rewrite::visit_all(tree, |node| {
        if node.is_named()
            && node.kind() == "import_spec"
            && let Some(path_node) = node.child_by_field_name("path")
            && let Ok(path_text) = path_node.utf8_text(source.as_bytes())
            && path_text.trim_matches('"') == target
        {
            visit(node);
        }
    });
}

/// Every import path in `source`, for near-miss reporting.
pub(crate) fn import_paths(source: &str) -> Vec<String> {
    crate::parse(source)
        .map(|tree| {
            colab_rewrite::field_texts(&tree, source, &["import_spec"], Some("path"), &['"'])
        })
        .unwrap_or_default()
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

    fn prefilter(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        import_paths(source)
    }
}

/// Replace every Go import whose path equals `from` with `to`.
///
/// Returns `source_code` unchanged if no imports match.
pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
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

    colab_rewrite::apply_edits(
        source_code,
        edits
            .into_iter()
            .map(|(start, end, text)| colab_rewrite::Edit::new(start, end, text))
            .collect(),
    )
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

    fn prefilter(&self) -> Option<&str> {
        Some(&self.target)
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.target)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        import_paths(source)
    }
}

/// Remove every Go import whose path equals `target`, leaving what
/// `gofmt` would: a group loses the spec's line and any blank line that
/// leaves dangling (after `(`, before `)`, or doubled), and a declaration
/// left with no specs is removed together with one surrounding blank line.
pub fn delete(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut edits = Vec::new();
    for decl in import_declarations(&tree) {
        let specs = specs_of(decl);
        let matched: Vec<Node<'_>> = specs
            .iter()
            .copied()
            .filter(|spec| spec_path(*spec, source_code) == Some(target))
            .collect();
        if matched.is_empty() {
            continue;
        }
        if matched.len() == specs.len() {
            edits.push(remove_declaration(decl, source_code));
        } else if let Some(group) = Group::of(decl, source_code) {
            let removed: HashSet<usize> = matched
                .iter()
                .map(|spec| line_start(source_code, spec.start_byte()))
                .collect();
            let mut kept = Vec::new();
            let mut at = group.open_end;
            for line in source_code[group.open_end..group.close_start].split_inclusive('\n') {
                if !removed.contains(&at) {
                    kept.push(line);
                }
                at += line.len();
            }
            edits.push(colab_rewrite::Edit::new(
                group.open_end,
                group.close_start,
                tidy_blank_lines(&kept),
            ));
        } else {
            // A one-line group (`import ("a"; "b")`): drop just the lines.
            for spec in &matched {
                let start = line_start(source_code, spec.start_byte());
                let end = line_end(source_code, spec.end_byte());
                if !edits.iter().any(|e: &colab_rewrite::Edit| e.start == start) {
                    edits.push(colab_rewrite::Edit::new(start, end, String::new()));
                }
            }
        }
    }
    colab_rewrite::apply_edits(source_code, edits)
}

/// The top-level `import` declarations, in source order.
fn import_declarations(tree: &Tree) -> Vec<Node<'_>> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .filter(|n| n.kind() == "import_declaration")
        .collect()
}

/// The `import_spec`s of one declaration, grouped or not.
fn specs_of(decl: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = decl.walk();
    let mut out = Vec::new();
    for child in decl.named_children(&mut cursor) {
        match child.kind() {
            "import_spec" => out.push(child),
            "import_spec_list" => {
                let mut inner = child.walk();
                out.extend(
                    child
                        .named_children(&mut inner)
                        .filter(|n| n.kind() == "import_spec"),
                );
            }
            _ => {}
        }
    }
    out
}

/// The unquoted path of an `import_spec`.
fn spec_path<'s>(spec: Node<'_>, source: &'s str) -> Option<&'s str> {
    let path = spec.child_by_field_name("path")?;
    Some(source[path.byte_range()].trim_matches(|c| c == '"' || c == '`'))
}

fn line_start(source: &str, at: usize) -> usize {
    source[..at].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

/// The end of the line holding `at`, past its newline.
fn line_end(source: &str, at: usize) -> usize {
    source[at..]
        .find('\n')
        .map(|i| at + i + 1)
        .unwrap_or(source.len())
}

/// A declaration's whole lines, plus one blank line when it sits between
/// two (or opens the file), so no double blank line is left behind.
fn remove_declaration(decl: Node<'_>, source: &str) -> colab_rewrite::Edit {
    let start = line_start(source, decl.start_byte());
    let mut end = line_end(source, decl.end_byte());
    let blank_before = start == 0
        || source[..start - 1]
            .rsplit('\n')
            .next()
            .is_some_and(is_blank);
    let next = line_end(source, end.min(source.len()));
    if blank_before && end < source.len() && is_blank(&source[end..next]) {
        end = next;
    }
    colab_rewrite::Edit::new(start, end, String::new())
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// Group lines with no blank line first or last and none doubled.
fn tidy_blank_lines(lines: &[&str]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for &line in lines {
        if is_blank(line) && out.last().is_none_or(|l| is_blank(l)) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| is_blank(l)) {
        out.pop();
    }
    out.concat()
}

/// An `import ( … )` group laid out as `gofmt` does: `(` ends its line
/// and `)` starts its own, so the specs are whole lines in between.
struct Group {
    /// Start of the line after `(`.
    open_end: usize,
    /// Start of the line holding `)`.
    close_start: usize,
}

impl Group {
    fn of(decl: Node<'_>, source: &str) -> Option<Self> {
        let mut cursor = decl.walk();
        let list = decl
            .named_children(&mut cursor)
            .find(|n| n.kind() == "import_spec_list")?;
        let open = list.child(0).filter(|n| n.kind() == "(")?;
        let close = list
            .child(list.child_count().checked_sub(1)?)
            .filter(|n| n.kind() == ")")?;
        let open_end = line_end(source, open.end_byte());
        if !is_blank(&source[open.end_byte()..open_end]) || open_end > close.start_byte() {
            return None;
        }
        let close_start = line_start(source, close.start_byte());
        if !is_blank(&source[close_start..close.start_byte()]) {
            return None;
        }
        Some(Self {
            open_end,
            close_start,
        })
    }
}

/// The import path on one line of a group, if the line holds a spec.
fn line_path(line: &str) -> Option<&str> {
    let t = line.trim();
    if t.is_empty() || t.starts_with("//") || t.starts_with("/*") {
        return None;
    }
    let q = t.find(['"', '`'])?;
    let quote = t[q..].chars().next()?;
    let rest = &t[q + 1..];
    Some(&rest[..rest.find(quote)?])
}

/// Standard-library paths have no dot in their first element.
fn is_std(path: &str) -> bool {
    !path.split('/').next().unwrap_or(path).contains('.')
}

/// Add `target` to a group's lines the way `goimports` does: in sorted
/// position within the first blank-line-separated block of its kind
/// (standard library or not), or as a new block — first for the
/// standard library, last otherwise.
fn insert_into_group(interior: &str, target: &str) -> String {
    let lines: Vec<&str> = interior.split_inclusive('\n').collect();
    let indent: String = lines
        .iter()
        .find(|l| line_path(l).is_some())
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_else(|| "\t".to_string());
    let new_line = format!("{}\"{}\"\n", indent, target);

    // Blocks of consecutive non-blank lines, as index ranges.
    let mut blocks: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if is_blank(line) {
            continue;
        }
        match blocks.last_mut() {
            Some(block) if block.end == i => block.end = i + 1,
            _ => blocks.push(i..i + 1),
        }
    }
    let kind = is_std(target);
    let home = blocks.iter().find(|block| {
        let paths: Vec<&str> = lines[(*block).clone()]
            .iter()
            .filter_map(|l| line_path(l))
            .collect();
        !paths.is_empty() && paths.iter().all(|p| is_std(p) == kind)
    });

    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match home {
        Some(block) => {
            let at = block
                .clone()
                .find(|&i| line_path(lines[i]).is_some_and(|p| p > target))
                .unwrap_or(block.end);
            out.insert(at, new_line);
        }
        None if blocks.is_empty() => out.insert(0, new_line),
        None if kind => {
            out.insert(0, "\n".to_string());
            out.insert(0, new_line);
        }
        None => {
            if !out.last().is_some_and(|l| l.ends_with('\n')) {
                out.push("\n".to_string());
            }
            out.push("\n".to_string());
            out.push(new_line);
        }
    }
    out.concat()
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

    // No prefilter: `ensure` acts precisely when the target is absent.
}

/// Idempotently add `target` as a Go import if no `import_spec`
/// already references it, placed as `goimports` would: into the existing
/// `import ( … )` group, or by turning a lone `import "x"` into a group.
/// With no imports at all, a standalone `import "<target>"` follows the
/// `package` clause.
pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut already_present = false;
    for_each_matching_import(&tree, source_code, target, |_| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    let decls = import_declarations(&tree);
    if let Some(group) = decls.iter().find_map(|d| Group::of(*d, source_code)) {
        let interior = &source_code[group.open_end..group.close_start];
        let edit = colab_rewrite::Edit::new(
            group.open_end,
            group.close_start,
            insert_into_group(interior, target),
        );
        return colab_rewrite::apply_edits(source_code, vec![edit]);
    }
    let lone = decls.iter().find(|d| {
        let mut cursor = d.walk();
        d.named_children(&mut cursor)
            .all(|c| c.kind() != "import_spec_list")
    });
    if let Some(decl) = lone
        && let [spec] = specs_of(*decl)[..]
    {
        let existing = format!("\t{}\n", &source_code[spec.byte_range()]);
        let text = format!("import (\n{})", insert_into_group(&existing, target));
        let edit = colab_rewrite::Edit::new(decl.start_byte(), decl.end_byte(), text);
        return colab_rewrite::apply_edits(source_code, vec![edit]);
    }

    // No imports yet: insert immediately after the package clause's
    // terminating newline. If there is no package clause (uncommon —
    // partial Go file fragment), fall back to prepending.
    let root = tree.root_node();
    let mut insert_at: Option<usize> = None;
    for i in 0..root.named_child_count() {
        if let Some(child) = root.named_child(i as u32)
            && child.kind() == "package_clause"
        {
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

    #[test]
    fn delete_leaves_no_dangling_blank_line_in_a_group() {
        let go_code = "package main\n\nimport (\n\t\"fmt\"\n\n\t\"github.com/pkg/errors\"\n)\n";
        assert_eq!(
            delete("github.com/pkg/errors", go_code),
            "package main\n\nimport (\n\t\"fmt\"\n)\n"
        );
    }

    #[test]
    fn delete_collapses_the_blank_lines_around_a_removed_spec() {
        let go_code = "package main\n\nimport (\n\t\"fmt\"\n\n\t\"a.b/c\"\n\n\t\"x.y/z\"\n)\n";
        assert_eq!(
            delete("a.b/c", go_code),
            "package main\n\nimport (\n\t\"fmt\"\n\n\t\"x.y/z\"\n)\n"
        );
    }

    #[test]
    fn delete_removes_a_declaration_left_empty() {
        let grouped = "package main\n\nimport (\n\t\"a.b/c\"\n)\n\nfunc main() {}\n";
        assert_eq!(delete("a.b/c", grouped), "package main\n\nfunc main() {}\n");
        let lone = "package main\n\nimport \"a.b/c\"\n\nfunc main() {}\n";
        assert_eq!(delete("a.b/c", lone), "package main\n\nfunc main() {}\n");
    }

    // ---------- ensure ---------------------------------------------------

    #[test]
    fn ensure_joins_the_existing_group_in_sorted_position() {
        let go_code =
            "package main\n\nimport (\n\t\"bytes\"\n\t\"os\"\n\n\t\"github.com/pkg/errors\"\n)\n";
        assert_eq!(
            ensure("fmt", go_code),
            "package main\n\nimport (\n\t\"bytes\"\n\t\"fmt\"\n\t\"os\"\n\n\t\"github.com/pkg/errors\"\n)\n"
        );
        assert_eq!(
            ensure("golang.org/x/sync", go_code),
            "package main\n\nimport (\n\t\"bytes\"\n\t\"os\"\n\n\t\"github.com/pkg/errors\"\n\t\"golang.org/x/sync\"\n)\n"
        );
    }

    #[test]
    fn ensure_starts_a_new_block_when_no_block_is_of_its_kind() {
        let third_party = "package main\n\nimport (\n\t\"github.com/a/b\"\n)\n";
        assert_eq!(
            ensure("fmt", third_party),
            "package main\n\nimport (\n\t\"fmt\"\n\n\t\"github.com/a/b\"\n)\n"
        );
        let std_only = "package main\n\nimport (\n\t\"os\"\n)\n";
        assert_eq!(
            ensure("github.com/a/b", std_only),
            "package main\n\nimport (\n\t\"os\"\n\n\t\"github.com/a/b\"\n)\n"
        );
    }

    #[test]
    fn ensure_turns_a_lone_import_into_a_group() {
        let go_code = "package main\n\nimport \"os\"\n\nfunc main() {}\n";
        assert_eq!(
            ensure("fmt", go_code),
            "package main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n\nfunc main() {}\n"
        );
        let aliased = "package main\n\nimport f \"fmt\"\n";
        assert_eq!(
            ensure("os", aliased),
            "package main\n\nimport (\n\tf \"fmt\"\n\t\"os\"\n)\n"
        );
    }

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
