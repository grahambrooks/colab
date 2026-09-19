//! `json::key`: set, insert or delete an object member by JSON Pointer.
//!
//! The document is parsed with tree-sitter-json only to locate spans; the
//! edit itself is one splice of text, so nothing else in the file moves.
//!
//! - A new member goes after the object's last member, on its own line
//!   with the same indentation when members are one per line, or after a
//!   `, ` when the object is written on one line.
//! - Missing parent objects are created, written compactly on one line.
//! - A pointer step into an array must be an existing index; `set` and
//!   `insert` never grow arrays.

use std::fmt;
use std::path::Path;

use colab_core::{Error, Operation, Result};
use colab_rewrite::{Edit, apply_edits};
use serde_json::Value;
use tree_sitter::Node;

use crate::pointer;

#[derive(Debug, Clone)]
enum Mode {
    Set { text: String, value: Value },
    Insert { text: String, value: Value },
    Delete,
}

/// One `json::key` rule.
#[derive(Debug)]
pub struct MemberEdit {
    tokens: Vec<String>,
    raw_pointer: String,
    mode: Mode,
}

impl MemberEdit {
    pub fn set(pointer: &str, value: &str) -> Result<Self> {
        let (text, value) = parse_value(pointer, value)?;
        Self::build(pointer, Mode::Set { text, value })
    }

    pub fn insert(pointer: &str, value: &str) -> Result<Self> {
        let (text, value) = parse_value(pointer, value)?;
        Self::build(pointer, Mode::Insert { text, value })
    }

    pub fn delete(pointer: &str) -> Result<Self> {
        Self::build(pointer, Mode::Delete)
    }

    fn build(raw_pointer: &str, mode: Mode) -> Result<Self> {
        let tokens = pointer::parse(raw_pointer)
            .map_err(|problem| Error::Config(format!("json::key \"{raw_pointer}\": {problem}")))?;
        Ok(Self {
            tokens,
            raw_pointer: raw_pointer.to_string(),
            mode,
        })
    }
}

fn parse_value(pointer: &str, text: &str) -> Result<(String, Value)> {
    let text = text.trim();
    let value = serde_json::from_str(text).map_err(|error| {
        Error::Config(format!(
            "json::key \"{pointer}\": `{text}` is not a JSON value ({error})"
        ))
    })?;
    Ok((text.to_string(), value))
}

impl fmt::Display for MemberEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.mode {
            Mode::Set { text, .. } => write!(f, "json::key \"{}\" -> set {text}", self.raw_pointer),
            Mode::Insert { text, .. } => {
                write!(f, "json::key \"{}\" -> insert {text}", self.raw_pointer)
            }
            Mode::Delete => write!(f, "json::key \"{}\" -> delete", self.raw_pointer),
        }
    }
}

impl Operation for MemberEdit {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|e| e.to_str()) == Some("json")
    }

    fn apply(&self, source_code: &str) -> String {
        match self.edit(source_code) {
            Some(edit) => apply_edits(source_code, vec![edit]),
            None => source_code.to_string(),
        }
    }

    // `set` and `insert` act when the member is absent, and a key may be
    // written with escapes, so no literal is a safe necessary condition.
}

impl MemberEdit {
    /// The one splice this rule makes, or `None` when nothing changes.
    fn edit(&self, source: &str) -> Option<Edit> {
        let tree = crate::parse(source)?;
        let root = tree.root_node();
        // A file that does not parse cleanly is left alone.
        if root.has_error() {
            return None;
        }
        let mut node = root.named_children(&mut root.walk()).find(is_value)?;
        let (last, parents) = self.tokens.split_last()?;
        for (depth, token) in parents.iter().enumerate() {
            match child(node, token, source) {
                Some(Child::Member { value, .. } | Child::Element(value)) => node = value,
                Some(Child::Missing) => {
                    // Create the rest of the path as nested objects.
                    let (text, _) = match &self.mode {
                        Mode::Set { text, value } | Mode::Insert { text, value } => (text, value),
                        Mode::Delete => return None,
                    };
                    let mut nested = text.clone();
                    for key in self.tokens[depth + 1..].iter().rev() {
                        nested = format!("{{{}: {nested}}}", quote(key));
                    }
                    return insert_member(node, token, &nested, source);
                }
                None => return None,
            }
        }
        match (child(node, last, source)?, &self.mode) {
            (Child::Missing, Mode::Delete) => None,
            (Child::Missing, Mode::Set { text, .. } | Mode::Insert { text, .. }) => {
                insert_member(node, last, text, source)
            }
            (Child::Member { .. } | Child::Element(_), Mode::Insert { .. }) => None,
            (
                Child::Member { value, .. } | Child::Element(value),
                Mode::Set {
                    text,
                    value: wanted,
                },
            ) => {
                let current: Value = serde_json::from_str(text_of(value, source)).ok()?;
                if current == *wanted {
                    return None;
                }
                Some(Edit::replacing(
                    value,
                    reindent(text, indent_at(source, value.start_byte())),
                ))
            }
            (Child::Member { pair, .. }, Mode::Delete) => Some(delete_member(node, pair)),
            // Removing an array element is not a member edit.
            (Child::Element(_), Mode::Delete) => None,
        }
    }
}

enum Child<'t> {
    Member {
        pair: Node<'t>,
        value: Node<'t>,
    },
    Element(Node<'t>),
    /// The node is an object without this member.
    Missing,
}

fn is_value(node: &Node) -> bool {
    matches!(
        node.kind(),
        "object" | "array" | "string" | "number" | "true" | "false" | "null"
    )
}

/// The member `token` of an object, or element `token` of an array. `None`
/// when the node cannot hold it: a scalar, or an array index that is not a
/// number or is out of range.
fn child<'t>(node: Node<'t>, token: &str, source: &str) -> Option<Child<'t>> {
    match node.kind() {
        "object" => {
            let found = pairs(node).find(|pair| {
                pair.child_by_field_name("key")
                    .and_then(|key| serde_json::from_str::<String>(text_of(key, source)).ok())
                    .is_some_and(|key| key == token)
            });
            Some(match found {
                Some(pair) => Child::Member {
                    pair,
                    value: pair.child_by_field_name("value")?,
                },
                None => Child::Missing,
            })
        }
        "array" => {
            let index: usize = token.parse().ok()?;
            let element = node
                .named_children(&mut node.walk())
                .filter(is_value)
                .nth(index)?;
            Some(Child::Element(element))
        }
        _ => None,
    }
}

fn pairs(object: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut cursor = object.walk();
    object
        .named_children(&mut cursor)
        .filter(|n| n.kind() == "pair")
        .collect::<Vec<_>>()
        .into_iter()
}

fn text_of<'s>(node: Node<'_>, source: &'s str) -> &'s str {
    &source[node.start_byte()..node.end_byte()]
}

fn quote(key: &str) -> String {
    serde_json::to_string(key).expect("a string serialises")
}

/// The whitespace at the start of the line holding byte `at`.
fn indent_at(source: &str, at: usize) -> &str {
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = &source[line_start..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// A value's later lines indented to sit under a member at `indent`.
fn reindent(text: &str, indent: &str) -> String {
    let mut lines = text.lines();
    let mut out = lines.next().unwrap_or_default().to_string();
    for line in lines {
        out.push('\n');
        out.push_str(indent);
        out.push_str(line);
    }
    out
}

/// The indentation one level in: the file's first indented line decides
/// the unit (two spaces when nothing is indented yet).
fn indent_unit(source: &str) -> &str {
    source
        .lines()
        .map(|line| &line[..line.len() - line.trim_start_matches([' ', '\t']).len()])
        .find(|indent| !indent.is_empty())
        .unwrap_or("  ")
}

fn insert_member(object: Node<'_>, key: &str, value: &str, source: &str) -> Option<Edit> {
    if object.kind() != "object" {
        return None;
    }
    let members: Vec<Node> = pairs(object).collect();
    match members.last() {
        Some(last) => {
            let one_line = object.start_position().row == last.start_position().row;
            let at = last.end_byte();
            let text = if one_line {
                format!(", {}: {value}", quote(key))
            } else {
                let indent = indent_at(source, last.start_byte());
                format!(",\n{indent}{}: {}", quote(key), reindent(value, indent))
            };
            Some(Edit::new(at, at, text))
        }
        None => {
            // An empty object: open it up onto its own lines.
            let base = indent_at(source, object.start_byte());
            let inner = format!("{base}{}", indent_unit(source));
            let text = format!(
                "{{\n{inner}{}: {}\n{base}}}",
                quote(key),
                reindent(value, &inner)
            );
            Some(Edit::new(object.start_byte(), object.end_byte(), text))
        }
    }
}

/// Removes a member with the comma that separates it from a neighbour.
fn delete_member(object: Node<'_>, pair: Node<'_>) -> Edit {
    let members: Vec<Node> = pairs(object).collect();
    let index = members
        .iter()
        .position(|m| m.id() == pair.id())
        .unwrap_or(0);
    if let Some(next) = members.get(index + 1) {
        // Up to the next member, taking the comma and the line break.
        return Edit::new(pair.start_byte(), next.start_byte(), "");
    }
    if index > 0 {
        // The last member: from the end of the previous one.
        return Edit::new(members[index - 1].end_byte(), pair.end_byte(), "");
    }
    // The only member: the object becomes `{}`.
    Edit::new(object.start_byte(), object.end_byte(), "{}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: &str = "{\n  \"permissions\": {\n    \"allow\": [\"Bash(ls:*)\"]\n  },\n  \"enabledPlugins\": {\n    \"mine@elsewhere\": true\n  }\n}\n";

    fn run(op: &MemberEdit, source: &str) -> String {
        let once = op.apply(source);
        assert_eq!(op.apply(&once), once, "not idempotent: {op}");
        assert!(
            serde_json::from_str::<Value>(&once).is_ok(),
            "not valid JSON after {op}:\n{once}"
        );
        once
    }

    #[test]
    fn a_member_is_added_after_the_last_in_the_same_style() {
        let op =
            MemberEdit::set("/enabledPlugins/code-intelligence@gb-agent-skills", "true").unwrap();
        assert_eq!(
            run(&op, SETTINGS),
            "{\n  \"permissions\": {\n    \"allow\": [\"Bash(ls:*)\"]\n  },\n  \"enabledPlugins\": {\n    \"mine@elsewhere\": true,\n    \"code-intelligence@gb-agent-skills\": true\n  }\n}\n"
        );
    }

    #[test]
    fn missing_parents_are_created() {
        let op = MemberEdit::set(
            "/extraKnownMarketplaces/gb-agent-skills",
            r#"{"source": {"source": "github", "repo": "grahambrooks/gb-agent-skills"}}"#,
        )
        .unwrap();
        let out = run(&op, SETTINGS);
        assert!(
            out.contains("  },\n  \"extraKnownMarketplaces\": {\"gb-agent-skills\": {\"source\": {\"source\": \"github\", \"repo\": \"grahambrooks/gb-agent-skills\"}}}\n}"),
            "{out}"
        );
    }

    #[test]
    fn an_equal_value_however_written_is_left_alone() {
        let op = MemberEdit::set("/permissions", r#"{ "allow" : [ "Bash(ls:*)" ] }"#).unwrap();
        assert_eq!(op.apply(SETTINGS), SETTINGS);
    }

    #[test]
    fn set_replaces_a_different_value_in_place() {
        let op = MemberEdit::set("/enabledPlugins/mine@elsewhere", "false").unwrap();
        assert_eq!(
            run(&op, SETTINGS),
            SETTINGS.replace("\"mine@elsewhere\": true", "\"mine@elsewhere\": false")
        );
    }

    #[test]
    fn insert_never_overwrites() {
        let op = MemberEdit::insert("/enabledPlugins/mine@elsewhere", "false").unwrap();
        assert_eq!(op.apply(SETTINGS), SETTINGS);
    }

    #[test]
    fn delete_takes_the_separating_comma() {
        let first = MemberEdit::delete("/permissions").unwrap();
        assert_eq!(
            run(&first, SETTINGS),
            "{\n  \"enabledPlugins\": {\n    \"mine@elsewhere\": true\n  }\n}\n"
        );
        let last = MemberEdit::delete("/enabledPlugins").unwrap();
        assert_eq!(
            run(&last, SETTINGS),
            "{\n  \"permissions\": {\n    \"allow\": [\"Bash(ls:*)\"]\n  }\n}\n"
        );
        let only = MemberEdit::delete("/enabledPlugins/mine@elsewhere").unwrap();
        assert!(run(&only, SETTINGS).contains("\"enabledPlugins\": {}"));
        assert_eq!(
            MemberEdit::delete("/absent").unwrap().apply(SETTINGS),
            SETTINGS
        );
    }

    #[test]
    fn an_empty_document_object_is_opened_up() {
        let op = MemberEdit::set("/a", "1").unwrap();
        assert_eq!(run(&op, "{}\n"), "{\n  \"a\": 1\n}\n");
    }

    #[test]
    fn a_one_line_object_stays_on_one_line() {
        let op = MemberEdit::set("/b", "[1, 2]").unwrap();
        assert_eq!(run(&op, "{\"a\": 1}"), "{\"a\": 1, \"b\": [1, 2]}");
    }

    #[test]
    fn array_steps_use_existing_indexes_only() {
        let op = MemberEdit::set("/list/1/name", "\"b2\"").unwrap();
        let source = "{\"list\": [{\"name\": \"a\"}, {\"name\": \"b\"}]}";
        assert_eq!(
            run(&op, source),
            "{\"list\": [{\"name\": \"a\"}, {\"name\": \"b2\"}]}"
        );
        assert_eq!(
            MemberEdit::set("/list/5/name", "1").unwrap().apply(source),
            source
        );
    }

    #[test]
    fn escaped_keys_are_matched_by_their_value() {
        let op = MemberEdit::set("/a~1b", "2").unwrap();
        assert_eq!(run(&op, "{\"a\\/b\": 1}"), "{\"a\\/b\": 2}");
    }

    #[test]
    fn invalid_json_and_bad_values_are_left_or_rejected() {
        let op = MemberEdit::set("/a", "1").unwrap();
        assert_eq!(op.apply("{\"a\": }"), "{\"a\": }");
        assert!(MemberEdit::set("/a", "not json").is_err());
        assert!(MemberEdit::set("a", "1").is_err());
    }
}
