//! Operations on Ruby `require` / `require_relative`: rename, delete,
//! ensure.
//!
//! Ruby has no import *statement* — `require 'foo'` is an ordinary method
//! call. So unlike every other backend, matching here looks for a `call`
//! node whose method name is `require` or `require_relative` and whose
//! sole argument is a string literal, then compares the string's content.
//!
//! Two consequences worth knowing:
//!
//! - A computed require (`require File.join(dir, 'x')`) has no string
//!   literal and never matches. That is deliberate: colab cannot know
//!   what it resolves to.
//! - `require` and `require_relative` are distinct in the source but
//!   share a target space here — a rule matches the path regardless of
//!   which form is used, and rename preserves the form and the quote
//!   style already in the file.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Tree};

const REQUIRE_METHODS: &[&str] = &["require", "require_relative"];

/// For a `call` node that is a require, return the string node and the
/// path it names.
fn require_path<'a>(node: Node<'a>, source: &'a str) -> Option<(Node<'a>, &'a str)> {
    if node.kind() != "call" {
        return None;
    }
    // A require has no receiver — `Foo.require 'x'` is something else.
    if node.child_by_field_name("receiver").is_some() {
        return None;
    }
    let method = node.child_by_field_name("method")?;
    let method_text = method.utf8_text(source.as_bytes()).ok()?;
    if !REQUIRE_METHODS.contains(&method_text) {
        return None;
    }

    let args = node.child_by_field_name("arguments")?;
    let first = args.named_child(0)?;
    if first.kind() != "string" {
        return None;
    }
    // `string` wraps the quotes; `string_content` is the path itself.
    let content = (0..first.named_child_count())
        .filter_map(|i| first.named_child(i as u32))
        .find(|c| c.kind() == "string_content")?;
    let text = content.utf8_text(source.as_bytes()).ok()?;
    Some((content, text))
}

fn for_each_matching_require<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
where
    F: FnMut(Node<'_>, Node<'_>),
{
    colab_rewrite::visit_all(tree, |node| {
    if let Some((content_node, text)) = require_path(node, source)
        && text == target
    {
        visit(node, content_node);
    }
    });
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct RequireRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for RequireRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ruby::require \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for RequireRename {
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
    for_each_matching_require(&tree, source_code, from, |_call, content| {
        // Only the string *content* is replaced, so the surrounding
        // quote style is preserved.
        edits.push((content.start_byte(), content.end_byte()));
    });
    colab_rewrite::apply_edits(
        source_code,
        edits
            .into_iter()
            .map(|(start, end)| colab_rewrite::Edit::new(start, end, to))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct RequireDelete {
    pub target: String,
}

impl fmt::Display for RequireDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ruby::require \"{}\" -> delete", self.target)
    }
}

impl Operation for RequireDelete {
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
    for_each_matching_require(&tree, source_code, target, |call, _content| {
        // `delete_lines` widens each range to whole lines.
        spans.push((call.start_byte(), call.end_byte()));
    });
    colab_rewrite::delete_lines(source_code, spans)
}

// ---------------------------------------------------------------------------
// Ensure
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct RequireEnsure {
    pub target: String,
}

impl fmt::Display for RequireEnsure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ruby::require \"{}\" -> ensure", self.target)
    }
}

impl Operation for RequireEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(&self.target, source_code)
    }

    // No prefilter: `ensure` acts precisely when the target is absent.
}

/// Insert `require '<target>'` after the last existing require, else at
/// the top of the file (below any leading magic comments).
pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut already_present = false;
    for_each_matching_require(&tree, source_code, target, |_, _| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    let root = tree.root_node();
    let mut insert_at = None;
    for i in 0..root.named_child_count() {
        let Some(child) = root.named_child(i as u32) else {
            break;
        };
        if require_path(child, source_code).is_some() {
            let end = child.end_byte();
            insert_at = Some(
                source_code[end..]
                    .find('\n')
                    .map(|p| end + p + 1)
                    .unwrap_or(source_code.len()),
            );
        }
    }

    // No existing requires: sit below any leading comment block (a
    // `# frozen_string_literal: true` magic comment, typically).
    let pos = insert_at.unwrap_or_else(|| {
        let mut offset = 0usize;
        for line in source_code.split_inclusive('\n') {
            if line.trim_start().starts_with('#') || line.trim().is_empty() {
                offset += line.len();
            } else {
                break;
            }
        }
        offset
    });

    let insertion = format!("require '{}'\n", target);
    let mut out = String::with_capacity(source_code.len() + insertion.len());
    out.push_str(&source_code[..pos]);
    out.push_str(&insertion);
    out.push_str(&source_code[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "require 'old/lib'\nrequire_relative 'helpers'\n\nputs 'hi'\n";

    #[test]
    fn renames_a_require_path() {
        let out = rename("old/lib", "new/lib", SRC);
        assert!(out.contains("require 'new/lib'"), "got: {out}");
        assert!(out.contains("require_relative 'helpers'"), "others untouched");
    }

    #[test]
    fn renames_a_require_relative_path() {
        let out = rename("helpers", "support/helpers", SRC);
        assert!(out.contains("require_relative 'support/helpers'"), "got: {out}");
    }

    #[test]
    fn preserves_the_existing_quote_style() {
        let src = "require \"old/lib\"\n";
        let out = rename("old/lib", "new/lib", src);
        assert_eq!(out, "require \"new/lib\"\n");
    }

    #[test]
    fn does_not_match_a_substring_of_a_longer_path() {
        assert_eq!(rename("lib", "WRONG", SRC), SRC);
    }

    #[test]
    fn computed_requires_never_match() {
        let src = "require File.join(dir, 'old/lib')\n";
        assert_eq!(rename("old/lib", "new/lib", src), src);
    }

    #[test]
    fn a_method_named_require_on_a_receiver_is_not_a_require() {
        let src = "Kernel.require 'old/lib'\n";
        assert_eq!(rename("old/lib", "new/lib", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let once = rename("old/lib", "new/lib", SRC);
        let twice = rename("old/lib", "new/lib", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn deletes_the_whole_require_line() {
        let out = delete("old/lib", SRC);
        assert!(!out.contains("old/lib"));
        assert!(out.contains("require_relative 'helpers'"));
        assert!(out.contains("puts 'hi'"));
    }

    #[test]
    fn delete_is_idempotent() {
        let once = delete("old/lib", SRC);
        let twice = delete("old/lib", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn ensure_adds_after_the_existing_requires() {
        let out = ensure("extra", SRC);
        assert!(out.contains("require 'extra'"), "got: {out}");
        let last_existing = out.find("require_relative 'helpers'").unwrap();
        let added = out.find("require 'extra'").unwrap();
        assert!(last_existing < added, "got: {out}");
    }

    #[test]
    fn ensure_sits_below_a_magic_comment() {
        let src = "# frozen_string_literal: true\n\nputs 'hi'\n";
        let out = ensure("json", src);
        assert!(
            out.starts_with("# frozen_string_literal: true\n\nrequire 'json'\n"),
            "got: {out}"
        );
    }

    #[test]
    fn ensure_is_a_noop_when_present_in_either_form() {
        assert_eq!(ensure("old/lib", SRC), SRC);
        assert_eq!(ensure("helpers", SRC), SRC);
    }

    #[test]
    fn ensure_is_idempotent() {
        let once = ensure("extra", SRC);
        let twice = ensure("extra", &once);
        assert_eq!(once, twice);
    }
}
