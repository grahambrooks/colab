//! Operations on `#include` directives: rename, delete, ensure.
//!
//! tree-sitter exposes `preproc_include` with a `path` field that is
//! either a `system_lib_string` (`<stdio.h>`) or a `string_literal`
//! (`"local/lib.h"`). The delimiters are part of the node text, so
//! matching strips them first.
//!
//! **Match strings are the bare path**, without delimiters:
//! `"old/lib.h"` matches both `#include "old/lib.h"` and
//! `#include <old/lib.h>`. Rename preserves whichever style the source
//! used, so a codemod never silently converts a local include into a
//! system one.
//!
//! For `ensure`, where there is no existing directive to take the style
//! from, wrap the target in angle brackets to request the system form:
//! `ensure "<stdio.h>"` inserts `#include <stdio.h>`, while
//! `ensure "local.h"` inserts `#include "local.h"`.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Tree};

/// Strip `<...>` or `"..."` from a path node's text.
fn unquote(text: &str) -> &str {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'<' && last == b'>') || (first == b'"' && last == b'"') {
            return &text[1..text.len() - 1];
        }
    }
    text
}

/// `true` when the target requests the angle-bracket form explicitly.
fn wants_system_form(target: &str) -> bool {
    target.starts_with('<') && target.ends_with('>') && target.len() >= 2
}

/// The path node of a `preproc_include`, plus its unquoted text.
fn include_path<'a>(node: Node<'a>, source: &'a str) -> Option<(Node<'a>, &'a str)> {
    if node.kind() != "preproc_include" {
        return None;
    }
    let path = node.child_by_field_name("path")?;
    if !matches!(path.kind(), "system_lib_string" | "string_literal") {
        return None;
    }
    let text = path.utf8_text(source.as_bytes()).ok()?;
    Some((path, unquote(text)))
}

fn for_each_matching_include<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
where
    F: FnMut(Node<'_>, Node<'_>),
{
    let wanted = unquote(target);
    colab_rewrite::visit_all(tree, |node| {
        if let Some((path_node, text)) = include_path(node, source)
            && text == wanted
        {
            visit(node, path_node);
        }
    });
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct IncludeRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for IncludeRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "c::include \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for IncludeRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        // The bare path appears verbatim inside whichever delimiters
        // the source used, so its absence implies a no-op.
        Some(unquote(&self.from))
    }
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };
    let replacement = unquote(to);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for_each_matching_include(&tree, source_code, from, |_directive, path_node| {
        let Ok(existing) = path_node.utf8_text(source_code.as_bytes()) else {
            return;
        };
        // Preserve the delimiter style already in the source.
        let rewritten = if existing.starts_with('<') {
            format!("<{}>", replacement)
        } else {
            format!("\"{}\"", replacement)
        };
        edits.push((path_node.start_byte(), path_node.end_byte(), rewritten));
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

#[derive(Debug)]
pub struct IncludeDelete {
    pub target: String,
}

impl fmt::Display for IncludeDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "c::include \"{}\" -> delete", self.target)
    }
}

impl Operation for IncludeDelete {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        delete(&self.target, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(unquote(&self.target))
    }
}

pub fn delete(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for_each_matching_include(&tree, source_code, target, |directive, _path| {
        // `delete_lines` widens each range to whole lines.
        spans.push((directive.start_byte(), directive.end_byte()));
    });
    colab_rewrite::delete_lines(source_code, spans)
}

// ---------------------------------------------------------------------------
// Ensure
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct IncludeEnsure {
    pub target: String,
}

impl fmt::Display for IncludeEnsure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "c::include \"{}\" -> ensure", self.target)
    }
}

impl Operation for IncludeEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(&self.target, source_code)
    }

    // No prefilter: `ensure` acts precisely when the target is absent.
}

/// Insert `#include <target>` after the last existing include, or at the
/// top of the file when there is none.
pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut already_present = false;
    for_each_matching_include(&tree, source_code, target, |_, _| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    let directive = if wants_system_form(target) {
        format!("#include {}\n", target)
    } else {
        format!("#include \"{}\"\n", unquote(target))
    };

    // Prefer to sit with the other includes.
    let mut insert_at = 0usize;
    let root = tree.root_node();
    for i in 0..root.named_child_count() {
        let Some(child) = root.named_child(i as u32) else {
            break;
        };
        if child.kind() == "preproc_include" {
            let end = child.end_byte();
            // A `preproc_include` node already spans its terminating
            // newline, so only skip forward when it somehow does not —
            // otherwise the insertion lands a line too far down.
            insert_at = if source_code[..end].ends_with('\n') {
                end
            } else {
                source_code[end..]
                    .find('\n')
                    .map(|p| end + p + 1)
                    .unwrap_or(source_code.len())
            };
        }
    }

    let mut out = String::with_capacity(source_code.len() + directive.len());
    out.push_str(&source_code[..insert_at]);
    out.push_str(&directive);
    out.push_str(&source_code[insert_at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str =
        "#include <stdio.h>\n#include \"old/lib.h\"\n\nint main(void) { return 0; }\n";

    #[test]
    fn renames_a_quoted_include_and_keeps_the_quotes() {
        let out = rename("old/lib.h", "new/lib.h", SRC);
        assert!(out.contains("#include \"new/lib.h\""), "got: {out}");
        assert!(
            out.contains("#include <stdio.h>"),
            "other includes untouched"
        );
    }

    #[test]
    fn renames_a_system_include_and_keeps_the_brackets() {
        let out = rename("stdio.h", "stdlib.h", SRC);
        assert!(out.contains("#include <stdlib.h>"), "got: {out}");
    }

    #[test]
    fn a_bare_target_matches_either_delimiter_style() {
        // Same target text, both forms present, both rewritten.
        let src = "#include <x.h>\n#include \"x.h\"\n";
        let out = rename("x.h", "y.h", src);
        assert_eq!(out, "#include <y.h>\n#include \"y.h\"\n");
    }

    #[test]
    fn rename_does_not_match_a_substring_of_a_longer_path() {
        assert_eq!(rename("lib.h", "WRONG", SRC), SRC);
    }

    #[test]
    fn rename_is_idempotent() {
        let once = rename("old/lib.h", "new/lib.h", SRC);
        let twice = rename("old/lib.h", "new/lib.h", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn deletes_the_whole_directive_line() {
        let out = delete("old/lib.h", SRC);
        assert!(!out.contains("old/lib.h"));
        assert!(out.contains("#include <stdio.h>"));
        assert!(out.contains("int main"));
    }

    #[test]
    fn delete_is_idempotent() {
        let once = delete("old/lib.h", SRC);
        let twice = delete("old/lib.h", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn ensure_adds_a_quoted_include_after_the_existing_ones() {
        let out = ensure("extra.h", SRC);
        assert!(out.contains("#include \"extra.h\""), "got: {out}");
        let last_existing = out.find("old/lib.h").unwrap();
        let added = out.find("extra.h").unwrap();
        assert!(
            last_existing < added,
            "should follow existing includes: {out}"
        );
    }

    #[test]
    fn ensure_honours_an_explicit_system_form() {
        let out = ensure("<stdlib.h>", SRC);
        assert!(out.contains("#include <stdlib.h>"), "got: {out}");
    }

    #[test]
    fn ensure_is_a_noop_when_already_present_in_either_form() {
        assert_eq!(ensure("stdio.h", SRC), SRC);
        assert_eq!(ensure("<stdio.h>", SRC), SRC);
        assert_eq!(ensure("old/lib.h", SRC), SRC);
    }

    #[test]
    fn ensure_prepends_when_there_are_no_includes() {
        let src = "int main(void) { return 0; }\n";
        let out = ensure("<stdio.h>", src);
        assert!(out.starts_with("#include <stdio.h>\n"), "got: {out}");
    }

    #[test]
    fn ensure_is_idempotent() {
        let once = ensure("extra.h", SRC);
        let twice = ensure("extra.h", &once);
        assert_eq!(once, twice);
    }
}
