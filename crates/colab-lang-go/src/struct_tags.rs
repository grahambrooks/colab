//! Rewrite Go struct tag pairs.
//!
//! Struct tags look like `` `json:"old_name" yaml:"old"` ``; this
//! operation finds a single `<key>:"<value>"` pair inside any
//! field-declaration tag and rewrites it, leaving other pairs in the
//! same tag string untouched.
//!
//! The DSL match string is the conceptual key/value pair without
//! quotes around the value: `match go::struct_tag "json:old_name"
//! { replace "json:new_name" }` rewrites every `json:"old_name"`
//! occurrence inside any struct tag.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::TreeCursor;

#[derive(Debug)]
pub struct TagReplace {
    pub from: String,
    pub to: String,
}

impl fmt::Display for TagReplace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "go::struct_tag \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for TagReplace {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("go")
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }
}

/// Parse a `<key>:<value>` literal into its parts. Returns `None` if
/// the form is malformed (missing `:` separator).
fn split_pair(pair: &str) -> Option<(&str, &str)> {
    pair.split_once(':')
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let Some((from_key, from_value)) = split_pair(from) else {
        return source_code.to_string();
    };
    let Some((to_key, to_value)) = split_pair(to) else {
        return source_code.to_string();
    };

    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let needle = format!("{}:\"{}\"", from_key, from_value);
    let replacement = format!("{}:\"{}\"", to_key, to_value);

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut cursor = tree.walk();
    collect(&mut cursor, source_code, &needle, &replacement, &mut edits);

    if edits.is_empty() {
        return source_code.to_string();
    }
    edits.sort_by_key(|e| e.0);
    let mut out = source_code.to_string();
    for (start, end, replacement) in edits.iter().rev() {
        out.replace_range(*start..*end, replacement);
    }
    out
}

fn collect(
    cursor: &mut TreeCursor,
    source: &str,
    needle: &str,
    replacement: &str,
    edits: &mut Vec<(usize, usize, String)>,
) {
    let node = cursor.node();
    if node.is_named()
        && node.kind() == "field_declaration"
        && let Some(tag) = node.child_by_field_name("tag")
        && let Ok(tag_text) = tag.utf8_text(source.as_bytes())
    {
        // tag_text is the full literal including backticks: `key:"v"`.
        // Search for the `key:"value"` pair inside the inner content.
        // Multiple occurrences in one tag are unlikely (Go tags don't
        // repeat keys) but we handle them defensively.
        let inner_start = tag.start_byte() + 1; // skip opening backtick
        let inner_end = tag.end_byte() - 1; // skip closing backtick
        if inner_end <= inner_start {
            // proceed with traversal anyway
        } else {
            let inner = &source[inner_start..inner_end];
            let mut search_offset = 0;
            while let Some(rel) = inner[search_offset..].find(needle) {
                let abs = inner_start + search_offset + rel;
                edits.push((abs, abs + needle.len(), replacement.to_string()));
                search_offset += rel + needle.len();
            }
        }
        let _ = tag_text;
    }

    if cursor.goto_first_child() {
        loop {
            collect(cursor, source, needle, replacement, edits);
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
    fn renames_a_single_json_tag_value() {
        let src = "package demo\n\ntype User struct {\n\tName string `json:\"old_name\"`\n}\n";
        let out = rename("json:old_name", "json:new_name", src);
        assert!(out.contains("`json:\"new_name\"`"), "got: {out}");
        assert!(!out.contains("old_name"));
    }

    #[test]
    fn leaves_other_pairs_in_same_tag_alone() {
        let src =
            "package demo\n\ntype User struct {\n\tName string `json:\"old\" yaml:\"keep\"`\n}\n";
        let out = rename("json:old", "json:new", src);
        assert!(out.contains("`json:\"new\" yaml:\"keep\"`"), "got: {out}");
    }

    #[test]
    fn rewrites_in_multiple_fields() {
        let src = "package demo\n\ntype A struct { X int `json:\"old\"` }\ntype B struct { Y int `json:\"old\"` }\n";
        let out = rename("json:old", "json:new", src);
        assert!(out.matches("json:\"new\"").count() == 2, "got: {out}");
        assert!(!out.contains("json:\"old\""));
    }

    #[test]
    fn can_change_tag_key_and_value() {
        // Renaming `json:foo` to `protobuf:foo` swaps the key.
        let src = "package demo\n\ntype A struct { X int `json:\"foo\"` }\n";
        let out = rename("json:foo", "protobuf:foo", src);
        assert!(out.contains("`protobuf:\"foo\"`"), "got: {out}");
    }

    #[test]
    fn does_not_rewrite_on_value_mismatch() {
        let src = "package demo\n\ntype A struct { X int `json:\"different\"` }\n";
        assert_eq!(rename("json:foo", "json:bar", src), src);
    }

    #[test]
    fn does_not_match_tag_value_with_options() {
        // `json:"name,omitempty"` stores "name,omitempty" as the value;
        // a target of `json:name` must not partial-match.
        let src = "package demo\n\ntype A struct { X int `json:\"name,omitempty\"` }\n";
        assert_eq!(rename("json:name", "json:renamed", src), src);
    }

    #[test]
    fn does_not_touch_string_literals_outside_tags() {
        let src = "package demo\n\nvar s = `json:\"old\"`\nfunc f() { _ = s }\n";
        // `s` is a regular raw string literal, not a struct tag.
        assert_eq!(rename("json:old", "json:new", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "package demo\n\ntype A struct { X int `json:\"old\"` }\n";
        let once = rename("json:old", "json:new", src);
        let twice = rename("json:old", "json:new", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn returns_input_unchanged_for_malformed_pair() {
        let src = "package demo\n\ntype A struct { X int `json:\"old\"` }\n";
        assert_eq!(rename("noseparator", "json:new", src), src);
        assert_eq!(rename("json:old", "noseparator", src), src);
    }

    #[test]
    fn tag_replace_operation_is_relevant_for_go_files() {
        let op = TagReplace {
            from: "json:a".into(),
            to: "json:b".into(),
        };
        assert!(op.is_file_relevant(Path::new("foo.go")));
        assert!(!op.is_file_relevant(Path::new("foo.rs")));
    }
}
