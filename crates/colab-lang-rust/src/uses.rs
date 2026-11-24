//! Operations on Rust `use` declarations: rename, delete, ensure.
//!
//! Path matching is segment-wise: `from = "tokio"` matches `tokio`,
//! `tokio::sync::Mutex`, `tokio as t`, but never `my_tokio` or
//! `foo::tokio`. Rename rewrites the matched prefix; delete erases
//! the whole `use` line; ensure inserts a new `use <target>;` if no
//! existing declaration starts with the same path.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::Node;

/// If `text` begins with `from` followed by a path-segment boundary
/// (`::`, end of path, whitespace, or `;`), returns the byte length
/// of the matched prefix.
fn match_path_prefix(text: &str, from: &str) -> Option<usize> {
    if !text.starts_with(from) {
        return None;
    }
    let after = &text[from.len()..];
    if after.is_empty()
        || after.starts_with("::")
        || after.starts_with(' ')
        || after.starts_with('\t')
        || after.starts_with('\n')
        || after.starts_with(';')
    {
        Some(from.len())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Use trees: the paths a `use` spells, member by member
// ---------------------------------------------------------------------------

/// `prefix::rest`, either part possibly empty.
fn join(prefix: &str, rest: &str) -> String {
    match (prefix.is_empty(), rest.is_empty()) {
        (true, _) => rest.to_string(),
        (_, true) => prefix.to_string(),
        _ => format!("{prefix}::{rest}"),
    }
}

/// The first `::` segment of `target`: text every `use` naming a path
/// under `target` contains, even when the rest of the path is spread over
/// a brace group (`use tree_sitter::{Parser, Tree}` names
/// `tree_sitter::Parser`).
fn first_segment(target: &str) -> &str {
    target.split("::").next().unwrap_or(target)
}

/// What becomes of one node of a use tree.
enum Fate {
    /// It stays, with these edits inside it.
    Keep(Vec<colab_rewrite::Edit>),
    /// It goes, with its separator.
    Drop,
}

/// One walk of a `use` tree for `delete` (no `rename_to`) or `replace`.
struct UseTree<'a> {
    source: &'a str,
    target: &'a str,
    rename_to: Option<&'a str>,
    /// Whole `use` lines a rename moves a member out to: its new path is
    /// not under the brace group it was in.
    moved_out: Vec<String>,
}

impl<'a> UseTree<'a> {
    fn text(&self, node: Node<'_>) -> &'a str {
        let source: &'a str = self.source;
        node.utf8_text(source.as_bytes()).unwrap_or("")
    }

    fn hits(&self, path: &str) -> bool {
        match_path_prefix(path, self.target).is_some()
    }

    /// The fate of `node` (a use tree, inside a group whose path is `prefix`).
    fn walk(&mut self, node: Node<'_>, prefix: &str) -> Fate {
        match node.kind() {
            "scoped_use_list" => {
                let path = node.child_by_field_name("path");
                let full = join(prefix, path.map_or("", |p| self.text(p)));
                if self.hits(&full) {
                    return self.hit(path.unwrap_or(node), prefix, &full, None);
                }
                match node.child_by_field_name("list") {
                    Some(list) => self.list(list, &full, path),
                    None => Fate::Keep(Vec::new()),
                }
            }
            "use_list" => self.list(node, prefix, None),
            "use_as_clause" => {
                let Some(path) = node.child_by_field_name("path") else {
                    return Fate::Keep(Vec::new());
                };
                let full = join(prefix, self.text(path));
                let alias = node.child_by_field_name("alias").map(|a| self.text(a));
                if self.hits(&full) {
                    self.hit(path, prefix, &full, alias)
                } else {
                    Fate::Keep(Vec::new())
                }
            }
            _ => {
                let text = self.text(node);
                let full = if text == "self" {
                    prefix.to_string()
                } else {
                    join(prefix, text)
                };
                if self.hits(&full) && text != "*" {
                    self.hit(node, prefix, &full, None)
                } else if text.ends_with('*') && self.hits(full.trim_end_matches("::*")) {
                    // `use a::*` names everything under `a`.
                    self.hit(node, prefix, &full, None)
                } else {
                    Fate::Keep(Vec::new())
                }
            }
        }
    }

    /// A member whose path `full` (written as `path_node`) is the target or
    /// under it.
    fn hit(&mut self, path_node: Node<'_>, prefix: &str, full: &str, alias: Option<&str>) -> Fate {
        let Some(to) = self.rename_to else {
            return Fate::Drop;
        };
        let renamed = format!("{to}{}", &full[self.target.len()..]);
        // `self` names the group's own path: only the group's path can
        // change, which the caller handles as a whole-group hit.
        if self.text(path_node) == "self" {
            return Fate::Keep(Vec::new());
        }
        let edit = |text: &str| {
            colab_rewrite::Edit::new(path_node.start_byte(), path_node.end_byte(), text)
        };
        if prefix.is_empty() {
            return Fate::Keep(vec![edit(&renamed)]);
        }
        match renamed.strip_prefix(&format!("{prefix}::")) {
            Some(rest) => Fate::Keep(vec![edit(rest)]),
            None => {
                // Under another path now: a `use` line of its own.
                let alias = alias.map_or_else(String::new, |a| format!(" as {a}"));
                self.moved_out.push(format!("use {renamed}{alias};"));
                Fate::Drop
            }
        }
    }

    /// The members of a brace group, `list`, under `prefix`. `path` is the
    /// group's own path node (`a` in `a::{b, c}`), absent for `use {b, c};`.
    fn list(&mut self, list: Node<'_>, prefix: &str, path: Option<Node<'_>>) -> Fate {
        let members: Vec<Node<'_>> = list
            .named_children(&mut list.walk())
            .filter(|m| !matches!(m.kind(), "line_comment" | "block_comment"))
            .collect();
        let fates: Vec<Fate> = members.iter().map(|m| self.walk(*m, prefix)).collect();
        let dropped: Vec<bool> = fates.iter().map(|f| matches!(f, Fate::Drop)).collect();
        if members.is_empty() || dropped.iter().all(|d| *d) {
            return if members.is_empty() {
                Fate::Keep(Vec::new())
            } else {
                Fate::Drop
            };
        }
        let mut edits: Vec<colab_rewrite::Edit> = Vec::new();
        if dropped.iter().any(|d| *d) {
            let kept: Vec<usize> = (0..members.len()).filter(|&i| !dropped[i]).collect();
            // One member left, untouched: no braces around it.
            if let [only] = kept[..]
                && matches!(&fates[only], Fate::Keep(inner) if inner.is_empty())
            {
                let text = self.text(members[only]);
                let collapsed = match path {
                    Some(path) if text == "self" => {
                        colab_rewrite::Edit::new(path.end_byte(), list.end_byte(), "")
                    }
                    _ => colab_rewrite::Edit::new(list.start_byte(), list.end_byte(), text),
                };
                return Fate::Keep(vec![collapsed]);
            }
            // Each run of dropped members goes with the commas around it:
            // up to the next member, or from the one before when it ends
            // the list.
            let mut i = 0;
            while i < members.len() {
                if !dropped[i] {
                    i += 1;
                    continue;
                }
                let run_start = i;
                while i < members.len() && dropped[i] {
                    i += 1;
                }
                let range = if i < members.len() {
                    (members[run_start].start_byte(), members[i].start_byte())
                } else {
                    (members[run_start - 1].end_byte(), members[i - 1].end_byte())
                };
                edits.push(colab_rewrite::Edit::new(range.0, range.1, ""));
            }
        }
        for fate in fates {
            if let Fate::Keep(inner) = fate {
                edits.extend(inner);
            }
        }
        Fate::Keep(edits)
    }
}

/// Every path a `use` declaration names, one per member, as written:
/// `use a::{b, c as d}` names `a::b` and `a::c`.
fn member_paths(source: &str, arg: Node<'_>, prefix: &str, out: &mut Vec<String>) {
    let text = |n: Node<'_>| n.utf8_text(source.as_bytes()).unwrap_or("").to_string();
    match arg.kind() {
        "scoped_use_list" => {
            let full = join(
                prefix,
                &arg.child_by_field_name("path").map_or(String::new(), text),
            );
            if let Some(list) = arg.child_by_field_name("list") {
                member_paths(source, list, &full, out);
            }
        }
        "use_list" => {
            for member in arg.named_children(&mut arg.walk()) {
                member_paths(source, member, prefix, out);
            }
        }
        "use_as_clause" => {
            if let Some(path) = arg.child_by_field_name("path") {
                out.push(join(prefix, &text(path)));
            }
        }
        "line_comment" | "block_comment" => {}
        _ => {
            let t = text(arg);
            out.push(if t == "self" {
                prefix.to_string()
            } else {
                join(prefix, &t)
            });
        }
    }
}

/// Rewrite every `use` in `source_code` that names `target` or a path
/// under it: drop those members (`rename_to` absent), or rename them.
fn rewrite_uses(target: &str, rename_to: Option<&str>, source_code: &str) -> String {
    let Some(tree) = crate::LANG.parse(source_code) else {
        return source_code.to_string();
    };
    let mut edits: Vec<colab_rewrite::Edit> = Vec::new();
    colab_rewrite::visit_all(&tree, |node| {
        if node.kind() != "use_declaration" {
            return;
        }
        let Some(arg) = node.child_by_field_name("argument") else {
            return;
        };
        let mut walk = UseTree {
            source: source_code,
            target,
            rename_to,
            moved_out: Vec::new(),
        };
        let fate = walk.walk(arg, "");
        // Lines moved out go after the declaration, with its indentation.
        if !walk.moved_out.is_empty() {
            let line_start = source_code[..node.start_byte()]
                .rfind('\n')
                .map_or(0, |p| p + 1);
            let indent: String = source_code[line_start..]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let lines: String = walk
                .moved_out
                .iter()
                .map(|l| format!("\n{indent}{l}"))
                .collect();
            edits.push(colab_rewrite::Edit::new(
                node.end_byte(),
                node.end_byte(),
                lines,
            ));
        }
        match fate {
            // The whole declaration goes: its line, when it has the line to
            // itself (the lines moved out go in its place).
            Fate::Drop => {
                let (start, end) =
                    colab_rewrite::line_span(source_code, node.start_byte(), node.end_byte());
                let alone = source_code[start..node.start_byte()].trim().is_empty()
                    && source_code[node.end_byte()..end].trim().is_empty();
                let (start, end) = if alone {
                    (start, end)
                } else {
                    (node.start_byte(), node.end_byte())
                };
                if walk.moved_out.is_empty() {
                    edits.push(colab_rewrite::Edit::new(start, end, ""));
                } else {
                    // Replace it with what moved out, rather than leave an
                    // edit at its end inside the range that goes.
                    edits.pop();
                    let indent: String = source_code[start..node.start_byte()].to_string();
                    let lines = walk
                        .moved_out
                        .iter()
                        .map(|l| format!("{indent}{l}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let tail = if alone && end > node.end_byte() {
                        "\n"
                    } else {
                        ""
                    };
                    edits.push(colab_rewrite::Edit::new(
                        start,
                        end,
                        format!("{lines}{tail}"),
                    ));
                }
            }
            Fate::Keep(inner) => edits.extend(inner),
        }
    });
    colab_rewrite::apply_edits(source_code, edits)
}

/// The argument text of every `use` declaration in `source`, for
/// near-miss reporting. The matcher segment-prefix-matches the target
/// against this same text.
pub(crate) fn use_paths(source: &str) -> Vec<String> {
    crate::LANG.parse(source).map_or_default(|tree| {
        colab_rewrite::field_texts(&tree, source, &["use_declaration"], Some("argument"), &[])
    })
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

/// `Operation` that renames a leading path prefix in `use` declarations.
#[derive(Debug)]
pub struct UseRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for UseRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rust::use \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for UseRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::LANG.is_source(path)
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }

    /// The path's first segment: a brace group spreads the rest of it
    /// (`use a::{b, c}` names `a::b`, which never appears whole).
    fn prefilter(&self) -> Option<&str> {
        Some(first_segment(&self.from))
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        use_paths(source)
    }
}

/// Rewrite the source: every path a `use` names that starts with segments
/// equal to `from` gets `to` in place of that prefix. A member of a brace
/// group is renamed inside the group when its new path is still under it
/// (`use a::{b, c}`, `a::b` → `a::d`: `use a::{d, c}`), and otherwise moves
/// to a `use` line of its own after the declaration.
pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    rewrite_uses(from, Some(to), source_code)
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// `Operation` that removes `use` declarations whose leading path
/// segment-matches `target`.
#[derive(Debug)]
pub struct UseDelete {
    pub target: String,
}

impl fmt::Display for UseDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rust::use \"{}\" -> delete", self.target)
    }
}

impl Operation for UseDelete {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::LANG.is_source(path)
    }

    fn apply(&self, source_code: &str) -> String {
        delete(&self.target, source_code)
    }

    /// The path's first segment, as for [`UseRename`].
    fn prefilter(&self) -> Option<&str> {
        Some(first_segment(&self.target))
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.target)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        use_paths(source)
    }
}

/// Remove every path a `use` names that is `target` or under it. A
/// declaration that names nothing else goes, line and all; otherwise only
/// those members go, with their commas, and a brace group left with one
/// member loses its braces (`use a::{b, c};` without `a::b`: `use a::c;`).
pub fn delete(target: &str, source_code: &str) -> String {
    rewrite_uses(target, None, source_code)
}

// ---------------------------------------------------------------------------
// Ensure
// ---------------------------------------------------------------------------

/// `Operation` that idempotently adds a `use <target>;` declaration
/// if no existing `use` already begins with the same path.
#[derive(Debug)]
pub struct UseEnsure {
    pub target: String,
}

impl fmt::Display for UseEnsure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rust::use \"{}\" -> ensure", self.target)
    }
}

impl Operation for UseEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::LANG.is_source(path)
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(&self.target, source_code)
    }

    // No prefilter: `ensure` acts precisely when the target is absent.
}

/// Insert `use <target>;` at the top of `source_code` if no existing
/// `use` declaration leads with the same path. The new line goes
/// after any leading inner attributes (`#![...]`) and inner doc
/// comments (`//!`, `/*! */`), which must come before every item.
pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::LANG.parse(source_code) else {
        return source_code.to_string();
    };

    // Present when any path a `use` names leads with it, a brace group's
    // members included: `use std::{fmt, io};` already names `std::fmt`, and a
    // second `use std::fmt;` would not build (E0252).
    let mut paths = Vec::new();
    colab_rewrite::visit_all(&tree, |node| {
        if node.kind() == "use_declaration"
            && let Some(arg) = node.child_by_field_name("argument")
        {
            member_paths(source_code, arg, "", &mut paths);
        }
    });
    if paths.iter().any(|p| match_path_prefix(p, target).is_some()) {
        return source_code.to_string();
    }

    let root = tree.root_node();
    // With `use` declarations already at the top level, join their group:
    // before the first one, unless an attribute sits on it (`#[cfg(test)]
    // use …;` would then cover the new line), in which case the next.
    let first_use = root
        .named_children(&mut root.walk())
        .filter(|c| c.kind() == "use_declaration")
        .find(|c| {
            c.prev_named_sibling()
                .is_none_or(|p| p.kind() != "attribute_item")
        });
    if let Some(first) = first_use {
        let at = colab_rewrite::line_span(source_code, first.start_byte(), first.end_byte()).0;
        let insertion = format!("use {target};\n");
        return format!("{}{insertion}{}", &source_code[..at], &source_code[at..]);
    }

    // Insert after the last leading inner attribute or inner doc
    // comment: both must precede every item. Plain comments among them
    // (a licence header) are looked past without moving the point. With
    // none, insert at byte 0.
    let mut insert_at = 0usize;
    for child in root.named_children(&mut root.walk()) {
        let comment = matches!(child.kind(), "line_comment" | "block_comment");
        let inner = child.kind() == "inner_attribute_item"
            || (comment && child.child_by_field_name("inner").is_some());
        if inner {
            let end = child.end_byte();
            insert_at = if source_code[..end].ends_with('\n') {
                end
            } else {
                source_code[end..].find('\n').map_or(end, |p| end + p + 1)
            };
        } else if !comment {
            break;
        }
    }

    // After the module's docs, set off by a blank line on each side, as
    // rustfmt leaves a file: `//! …`, blank, `use …;`, blank, the items.
    let insertion = if insert_at > 0 && source_code[insert_at..].starts_with('\n') {
        format!("\nuse {target};\n")
    } else if insert_at > 0 && !source_code[insert_at..].is_empty() {
        format!("\nuse {target};\n\n")
    } else {
        format!("use {target};\n")
    };
    let mut out = String::with_capacity(source_code.len() + insertion.len());
    out.push_str(&source_code[..insert_at]);
    out.push_str(&insertion);
    out.push_str(&source_code[insert_at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_targets_lists_every_use_argument() {
        let src = "use std::fmt;\nuse tokio::sync::{Mutex, RwLock};\n";
        assert_eq!(
            use_paths(src),
            vec!["std::fmt", "tokio::sync::{Mutex, RwLock}"]
        );
    }

    // ---------- rename ---------------------------------------------------

    #[test]
    fn renames_simple_use_path() {
        let src = "use tokio::sync::Mutex;\n";
        let out = rename("tokio", "async_tokio", src);
        assert_eq!(out, "use async_tokio::sync::Mutex;\n");
    }

    #[test]
    fn does_not_substring_match_first_segment() {
        let src = "use my_tokio::sync;\n";
        assert_eq!(rename("tokio", "WRONG", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "use tokio::sync::Mutex;\nuse tokio::fs;\n";
        let once = rename("tokio", "async_tokio", src);
        let twice = rename("tokio", "async_tokio", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn use_rename_operation_is_relevant_for_rs_files_only() {
        let op = UseRename {
            from: "a".into(),
            to: "b".into(),
        };
        assert!(op.is_file_relevant(Path::new("foo.rs")));
        assert!(!op.is_file_relevant(Path::new("foo.go")));
        assert!(!op.is_file_relevant(Path::new("Cargo.toml")));
    }

    // ---------- delete ---------------------------------------------------

    #[test]
    fn deletes_matching_use_lines() {
        let src = "use tokio::sync::Mutex;\nuse std::fmt;\nuse tokio::fs;\n";
        let out = delete("tokio", src);
        assert!(!out.contains("tokio"));
        assert!(out.contains("use std::fmt;"));
    }

    #[test]
    fn delete_does_not_match_substrings() {
        let src = "use my_tokio::sync;\n";
        assert_eq!(delete("tokio", src), src);
    }

    #[test]
    fn delete_is_idempotent() {
        let src = "use tokio::sync::Mutex;\nuse std::fmt;\n";
        let once = delete("tokio", src);
        let twice = delete("tokio", &once);
        assert_eq!(once, twice);
    }

    // ---------- brace groups ---------------------------------------------

    #[test]
    fn delete_takes_one_member_of_a_brace_group() {
        // Two left: they keep their braces.
        assert_eq!(delete("a::b", "use a::{b, c, d};\n"), "use a::{c, d};\n");
        // The last member, with the comma before it.
        assert_eq!(delete("a::d", "use a::{b, c, d};\n"), "use a::{b, c};\n");
        // One left: no braces around it.
        assert_eq!(
            delete("tree_sitter::Parser", "use tree_sitter::{Parser, Tree};\n"),
            "use tree_sitter::Tree;\n"
        );
        assert_eq!(delete("a::b", "use a::{self, b};\n"), "use a;\n");
        // Every member: the whole declaration, line and all.
        assert_eq!(
            delete("a::b", "use a::{b};\nfn main() {}\n"),
            "fn main() {}\n"
        );
        // An alias and a nested group.
        assert_eq!(
            delete("a::b", "use a::{b as x, c::{d, e}};\n"),
            "use a::c::{d, e};\n"
        );
        assert_eq!(
            delete("a::c::d", "use a::{b, c::{d, e}};\n"),
            "use a::{b, c::e};\n"
        );
        // The group's own path still deletes it all, as before.
        assert_eq!(delete("a", "use a::{b, c};\nuse z;\n"), "use z;\n");
        // A member that only shares a prefix is left alone.
        assert_eq!(delete("a::b", "use a::{bc, d};\n"), "use a::{bc, d};\n");
    }

    #[test]
    fn delete_of_a_member_keeps_a_multi_line_group_tidy() {
        let src = "use a::{\n    b,\n    c,\n    d,\n};\n";
        assert_eq!(delete("a::c", src), "use a::{\n    b,\n    d,\n};\n");
        assert_eq!(delete("a::d", src), "use a::{\n    b,\n    c,\n};\n");
    }

    #[test]
    fn delete_of_a_member_is_idempotent() {
        let once = delete("a::b", "use a::{b, c, d};\n");
        assert_eq!(delete("a::b", &once), once);
    }

    #[test]
    fn rename_edits_a_member_in_place_or_moves_it_out() {
        // Still under the group: renamed where it is.
        assert_eq!(
            rename("a::b", "a::x", "use a::{b, c};\n"),
            "use a::{x, c};\n"
        );
        assert_eq!(
            rename("a::b", "a::x", "use a::{b as y, c};\n"),
            "use a::{x as y, c};\n"
        );
        // Under another path: a `use` line of its own, alias kept.
        assert_eq!(
            rename("a::b", "z::b", "use a::{b as y, c};\n"),
            "use a::c;\nuse z::b as y;\n"
        );
        // Every member moved out: only the new lines are left.
        assert_eq!(rename("a::b", "z::b", "use a::{b};\n"), "use z::b;\n");
        // The group's own path: renamed as a prefix, as before.
        assert_eq!(rename("a", "q", "use a::{b, c};\n"), "use q::{b, c};\n");
    }

    #[test]
    fn ensure_sees_a_member_of_a_brace_group() {
        let src = "use std::{fmt, io};\n";
        assert_eq!(ensure("std::fmt", src), src);
    }

    #[test]
    fn the_prefilter_holds_for_a_path_spread_over_braces() {
        let op = UseDelete {
            target: "tree_sitter::Parser".into(),
        };
        let src = "use tree_sitter::{Parser, Tree};\n";
        let literal = op.prefilter().unwrap();
        assert!(src.contains(literal), "{literal}");
        assert_ne!(op.apply(src), src);
    }

    // ---------- ensure ---------------------------------------------------

    #[test]
    fn ensure_inserts_when_missing() {
        let src = "fn main() {}\n";
        let out = ensure("std::fmt", src);
        assert!(out.starts_with("use std::fmt;\n"), "got: {out}");
    }

    #[test]
    fn ensure_is_noop_when_path_already_imported() {
        let src = "use std::fmt::Write;\n\nfn main() {}\n";
        // Existing `use` starts with "std::fmt", which segment-matches
        // ensuring "std::fmt".
        assert_eq!(ensure("std::fmt", src), src);
    }

    #[test]
    fn ensure_inserts_after_inner_attributes() {
        let src = "#![allow(dead_code)]\nfn main() {}\n";
        let out = ensure("std::fmt", src);
        assert_eq!(
            out,
            "#![allow(dead_code)]\n\nuse std::fmt;\n\nfn main() {}\n"
        );
    }

    #[test]
    fn ensure_joins_the_existing_use_group() {
        // `//!` docs must precede every item; a `use` above them is E0753.
        let src = "//! Crate docs.\n// licence\n#![allow(dead_code)]\n\nuse std::io;\n";
        assert_eq!(
            ensure("std::fmt", src),
            "//! Crate docs.\n// licence\n#![allow(dead_code)]\n\nuse std::fmt;\nuse std::io;\n"
        );
        // Never under an attribute that belongs to another `use`.
        let gated = "#[cfg(test)]\nuse a::b;\nuse c::d;\n";
        assert_eq!(
            ensure("std::fmt", gated),
            "#[cfg(test)]\nuse a::b;\nuse std::fmt;\nuse c::d;\n"
        );
    }

    #[test]
    fn ensure_sets_the_use_off_from_inner_docs() {
        let docs = "//! Docs.\n\nfn main() {}\n";
        assert_eq!(
            ensure("std::fmt", docs),
            "//! Docs.\n\nuse std::fmt;\n\nfn main() {}\n"
        );
        let block = "/*! Docs. */\nfn main() {}\n";
        assert_eq!(
            ensure("std::fmt", block),
            "/*! Docs. */\n\nuse std::fmt;\n\nfn main() {}\n"
        );
    }

    #[test]
    fn ensure_is_idempotent() {
        let src = "fn main() {}\n";
        let once = ensure("std::fmt", src);
        let twice = ensure("std::fmt", &once);
        assert_eq!(once, twice);
    }
}
