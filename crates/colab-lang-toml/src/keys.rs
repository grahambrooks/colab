//! `toml::key`: set, insert or delete the key or array element a path
//! names, through `toml_edit`, so comments and formatting elsewhere in the
//! file survive.
//!
//! - `set` creates the target and any missing parents, and overwrites a
//!   value that differs. A missing selector parent (`repos[repo=local]`) is
//!   created as an element holding just its selector field.
//! - `insert` does the same only when the target is absent.
//! - `delete` removes the target; a missing target or parent is a no-op.
//!
//! Values are compared by meaning, not text, so re-running a `set` whose
//! value is already there, however formatted, changes nothing.

use std::fmt;
use std::path::Path;

use colab_core::{Error, Operation, Result};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use crate::path::{self, Segment};

#[derive(Debug, Clone)]
enum Mode {
    Set(Value),
    Insert(Value),
    Delete,
}

impl Mode {
    fn creates(&self) -> bool {
        !matches!(self, Mode::Delete)
    }
}

/// One `toml::key` rule.
#[derive(Debug)]
pub struct KeyEdit {
    path: Vec<Segment>,
    raw_path: String,
    raw_value: Option<String>,
    mode: Mode,
}

impl KeyEdit {
    pub fn set(path: &str, value: &str) -> Result<Self> {
        Self::build(path, Some(value), Mode::Set)
    }

    pub fn insert(path: &str, value: &str) -> Result<Self> {
        Self::build(path, Some(value), Mode::Insert)
    }

    pub fn delete(path: &str) -> Result<Self> {
        Self::build(path, None, |_| Mode::Delete)
    }

    fn build(path: &str, value: Option<&str>, mode: fn(Value) -> Mode) -> Result<Self> {
        let problem = |text: String| Error::Config(format!("toml::key \"{path}\": {text}"));
        let segments = path::parse(path).map_err(problem)?;
        let mode = match value {
            None => Mode::Delete,
            Some(text) => {
                let parsed = parse_value(text).map_err(problem)?;
                if let Some(Segment::Select { field, value, .. }) = segments.last() {
                    let names_itself = parsed
                        .as_inline_table()
                        .and_then(|table| table.get(field))
                        .is_some_and(|found| selects(found, value));
                    if !names_itself {
                        return Err(problem(format!(
                            "the value must be an inline table with {field} = \"{value}\", \
                             or the element could never be found again"
                        )));
                    }
                }
                mode(parsed)
            }
        };
        Ok(Self {
            path: segments,
            raw_path: path.to_string(),
            raw_value: value.map(str::to_string),
            mode,
        })
    }
}

/// A TOML value, as it would appear after `key = `.
fn parse_value(text: &str) -> std::result::Result<Value, String> {
    let document = format!("v = {}\n", text.trim())
        .parse::<DocumentMut>()
        .map_err(|error| {
            let first = error.to_string();
            let first = first
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("");
            format!("`{}` is not a TOML value ({first})", text.trim())
        })?;
    match document.get("v") {
        Some(Item::Value(value)) => Ok(value.clone()),
        _ => Err(format!("`{}` is not a TOML value", text.trim())),
    }
}

impl fmt::Display for KeyEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let action = match self.mode {
            Mode::Set(_) => "set",
            Mode::Insert(_) => "insert",
            Mode::Delete => "delete",
        };
        match &self.raw_value {
            Some(value) => write!(
                f,
                "toml::key \"{}\" -> {action} {}",
                self.raw_path,
                value.trim()
            ),
            None => write!(f, "toml::key \"{}\" -> {action}", self.raw_path),
        }
    }
}

impl Operation for KeyEdit {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|e| e.to_str()) == Some("toml")
    }

    fn apply(&self, source_code: &str) -> String {
        // A file that does not parse is left alone rather than guessed at.
        let Ok(mut document) = source_code.parse::<DocumentMut>() else {
            return source_code.to_string();
        };
        if edit(document.as_table_mut(), &self.path, &self.mode) {
            document.to_string()
        } else {
            source_code.to_string()
        }
    }

    // `set` and `insert` act when the target is absent, and a TOML key can
    // be spelled several ways (bare, quoted, dotted), so no literal is a
    // safe necessary condition.
}

/// Applies `mode` at `path` below `table`; true when something changed.
fn edit(table: &mut dyn TableLike, path: &[Segment], mode: &Mode) -> bool {
    let Some((first, rest)) = path.split_first() else {
        return false;
    };
    if rest.is_empty() {
        return edit_leaf(table, first, mode);
    }
    match first {
        Segment::Key(key) => {
            let mut created = false;
            if table.get(key).is_none() {
                if !mode.creates() {
                    return false;
                }
                let mut parent = Table::new();
                parent.set_implicit(true);
                table.insert(key, Item::Table(parent));
                created = true;
            }
            match table.get_mut(key).and_then(Item::as_table_like_mut) {
                Some(child) => edit(child, rest, mode) || created,
                // A scalar where a table is needed: nothing to descend into.
                None => created,
            }
        }
        Segment::Select { key, field, value } => {
            let (index, created) = match find(table, key, field, value) {
                Some(index) => (index, false),
                None if mode.creates() => {
                    let mut element = InlineTable::new();
                    element.insert(field, Value::from(value.as_str()));
                    match append(table, key, element) {
                        Some(index) => (index, true),
                        None => return false,
                    }
                }
                None => return false,
            };
            let changed = match table.get_mut(key) {
                Some(Item::ArrayOfTables(tables)) => tables
                    .get_mut(index)
                    .is_some_and(|child| edit(child, rest, mode)),
                Some(Item::Value(Value::Array(array))) => match array.get_mut(index) {
                    Some(Value::InlineTable(child)) => edit(child, rest, mode),
                    _ => false,
                },
                _ => false,
            };
            changed || created
        }
    }
}

fn edit_leaf(table: &mut dyn TableLike, segment: &Segment, mode: &Mode) -> bool {
    match segment {
        Segment::Key(key) => match mode {
            Mode::Set(value) => match table.get_mut(key) {
                Some(existing) if same_item(existing, &Item::Value(value.clone())) => false,
                Some(existing) => {
                    // In place, so the key keeps its position, and with the
                    // old value's surrounding whitespace and trailing comment.
                    let mut value = value.clone();
                    if let Item::Value(old) = existing {
                        *value.decor_mut() = old.decor().clone();
                    }
                    *existing = Item::Value(value);
                    true
                }
                None => {
                    table.insert(key, Item::Value(value.clone()));
                    true
                }
            },
            Mode::Insert(value) => {
                if table.get(key).is_some() {
                    return false;
                }
                table.insert(key, Item::Value(value.clone()));
                true
            }
            Mode::Delete => table.remove(key).is_some(),
        },
        Segment::Select { key, field, value } => {
            let found = find(table, key, field, value);
            match (mode, found) {
                (Mode::Delete, None) | (Mode::Insert(_), Some(_)) => false,
                (Mode::Delete, Some(index)) => {
                    match table.get_mut(key) {
                        Some(Item::ArrayOfTables(tables)) => {
                            tables.remove(index);
                        }
                        Some(Item::Value(Value::Array(array))) => {
                            array.remove(index);
                        }
                        _ => return false,
                    }
                    true
                }
                (Mode::Set(new) | Mode::Insert(new), None) => {
                    let Some(element) = new.as_inline_table() else {
                        return false;
                    };
                    append(table, key, element.clone()).is_some()
                }
                (Mode::Set(new), Some(index)) => {
                    let Some(element) = new.as_inline_table() else {
                        return false;
                    };
                    match table.get_mut(key) {
                        Some(Item::ArrayOfTables(tables)) => {
                            let Some(current) = tables.get_mut(index) else {
                                return false;
                            };
                            if same_table(current, element) {
                                return false;
                            }
                            let mut replacement = element.clone().into_table();
                            *replacement.decor_mut() = current.decor().clone();
                            *current = replacement;
                            true
                        }
                        Some(Item::Value(Value::Array(array))) => {
                            let Some(current) = array.get(index) else {
                                return false;
                            };
                            if same_value(current, new) {
                                return false;
                            }
                            // `replace` keeps the element's surrounding formatting.
                            array.replace(index, new.clone());
                            true
                        }
                        _ => false,
                    }
                }
            }
        }
    }
}

/// The index of the element of array `key` whose `field` equals `value`.
fn find(table: &dyn TableLike, key: &str, field: &str, value: &str) -> Option<usize> {
    let matches = |element: &dyn TableLike| {
        element
            .get(field)
            .and_then(Item::as_value)
            .is_some_and(|found| selects(found, value))
    };
    match table.get(key)? {
        Item::ArrayOfTables(tables) => tables.iter().position(|t| matches(t)),
        Item::Value(Value::Array(array)) => array.iter().position(|element| {
            element
                .as_inline_table()
                .is_some_and(|t| matches(t as &dyn TableLike))
        }),
        _ => None,
    }
}

/// Whether a selector value names this field value: a string by its text,
/// a number or boolean by how it is written.
fn selects(found: &Value, wanted: &str) -> bool {
    match found {
        Value::String(text) => text.value() == wanted,
        Value::Integer(number) => number.value().to_string() == wanted,
        Value::Boolean(flag) => flag.value().to_string() == wanted,
        _ => false,
    }
}

/// Appends `element` to array `key`, creating an array of tables when the
/// key is absent. A new inline element copies the last element's
/// surrounding whitespace, so a one-per-line array stays one per line.
fn append(table: &mut dyn TableLike, key: &str, element: InlineTable) -> Option<usize> {
    match table.get_mut(key) {
        None => {
            let mut tables = ArrayOfTables::new();
            tables.push(element.into_table());
            table.insert(key, Item::ArrayOfTables(tables));
            Some(0)
        }
        Some(Item::ArrayOfTables(tables)) => {
            tables.push(element.into_table());
            Some(tables.len() - 1)
        }
        Some(Item::Value(Value::Array(array))) => {
            let mut value = Value::InlineTable(element);
            // The new element takes the last one's leading whitespace, and
            // its trailing whitespace (the line break before `]`), so the
            // comma lands straight after the old last element.
            if let Some(last) = array.iter_mut().last() {
                let decor = last.decor().clone();
                last.decor_mut().set_suffix("");
                *value.decor_mut() = decor;
            }
            array.push_formatted(value);
            Some(array.len() - 1)
        }
        Some(_) => None,
    }
}

fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.value() == y.value(),
        (Value::Integer(x), Value::Integer(y)) => x.value() == y.value(),
        (Value::Float(x), Value::Float(y)) => x.value() == y.value(),
        (Value::Boolean(x), Value::Boolean(y)) => x.value() == y.value(),
        (Value::Datetime(x), Value::Datetime(y)) => x.value() == y.value(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same_value(p, q))
        }
        (Value::InlineTable(x), Value::InlineTable(y)) => same_table(x, y),
        _ => false,
    }
}

fn same_table(a: &dyn TableLike, b: &dyn TableLike) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|(key, item)| b.get(key).is_some_and(|other| same_item(item, other)))
}

fn same_item(a: &Item, b: &Item) -> bool {
    match (a, b) {
        (Item::Value(x), Item::Value(y)) => same_value(x, y),
        (Item::Table(x), Item::Table(y)) => same_table(x, y),
        (Item::Table(x), Item::Value(Value::InlineTable(y)))
        | (Item::Value(Value::InlineTable(y)), Item::Table(x)) => same_table(x, y),
        (Item::ArrayOfTables(x), Item::ArrayOfTables(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same_table(p, q))
        }
        (Item::ArrayOfTables(x), Item::Value(Value::Array(y)))
        | (Item::Value(Value::Array(y)), Item::ArrayOfTables(x)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|(p, q)| q.as_inline_table().is_some_and(|q| same_table(p, q)))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREK: &str = r#"# Git hooks, run by prek.

[[repos]]
repo = "local"
hooks = [
  {
    id = "gitleaks",
    entry = "gitleaks git --staged",
    stages = ["pre-commit"]
  },
  {
    id = "make-check",
    entry = "make check",
    stages = ["pre-push"]
  }
]
"#;

    fn run(op: &KeyEdit, source: &str) -> String {
        let once = op.apply(source);
        assert_eq!(op.apply(&once), once, "not idempotent: {op}");
        once
    }

    #[test]
    fn set_creates_a_key_and_its_parents() {
        let op = KeyEdit::set("project.profile_version", "2").unwrap();
        assert_eq!(
            run(&op, "# mine\n[commands]\ntest = [\"make\"]\n"),
            "# mine\n[commands]\ntest = [\"make\"]\n\n[project]\nprofile_version = 2\n"
        );
    }

    #[test]
    fn set_overwrites_in_place_and_keeps_comments() {
        let op = KeyEdit::set("project.profile_version", "3").unwrap();
        let source = "[project]\nname = \"x\" # the name\nprofile_version = 2 # bumped by migrations\nprofile = \"rust\"\n";
        assert_eq!(
            run(&op, source),
            "[project]\nname = \"x\" # the name\nprofile_version = 3 # bumped by migrations\nprofile = \"rust\"\n"
        );
    }

    #[test]
    fn an_equal_value_however_written_is_left_alone() {
        let op = KeyEdit::set("a", "{ x = 1, y = 'two' }").unwrap();
        let source = "a = {y = \"two\", x = 1}\n";
        assert_eq!(op.apply(source), source);
    }

    #[test]
    fn insert_never_overwrites() {
        let op = KeyEdit::insert("project.profile_version", "9").unwrap();
        let source = "[project]\nprofile_version = 2\n";
        assert_eq!(op.apply(source), source);
        assert_eq!(run(&op, "[project]\n"), "[project]\nprofile_version = 9\n");
    }

    #[test]
    fn delete_removes_and_a_missing_key_is_a_no_op() {
        let op = KeyEdit::delete("project.old").unwrap();
        assert_eq!(
            run(&op, "[project]\nold = 1\nkept = 2\n"),
            "[project]\nkept = 2\n"
        );
        assert_eq!(op.apply("[other]\nx = 1\n"), "[other]\nx = 1\n");
    }

    #[test]
    fn a_hook_is_added_to_an_inline_array_in_its_style() {
        let op = KeyEdit::insert(
            "repos[repo=local].hooks[id=cargo-fmt]",
            "{\n    id = \"cargo-fmt\",\n    entry = \"cargo fmt --all --check\",\n    stages = [\"pre-commit\"]\n  }",
        )
        .unwrap();
        let out = run(&op, PREK);
        assert!(out.starts_with("# Git hooks, run by prek.\n"), "{out}");
        assert!(
            out.contains("    stages = [\"pre-push\"]\n  },\n  {\n    id = \"cargo-fmt\",\n    entry = \"cargo fmt --all --check\",\n    stages = [\"pre-commit\"]\n  }\n]"),
            "{out}"
        );
        // Existing hooks are untouched.
        assert!(out.contains("entry = \"gitleaks git --staged\""));
    }

    #[test]
    fn set_replaces_a_hook_that_differs_and_keeps_the_rest() {
        let op = KeyEdit::set(
            "repos[repo=local].hooks[id=gitleaks]",
            "{ id = \"gitleaks\", entry = \"gitleaks git --staged --redact\", stages = [\"pre-commit\"] }",
        )
        .unwrap();
        let out = run(&op, PREK);
        assert!(out.contains("--redact"), "{out}");
        assert!(out.contains("id = \"make-check\""), "{out}");
    }

    #[test]
    fn delete_removes_one_hook() {
        let op = KeyEdit::delete("repos[repo=local].hooks[id=gitleaks]").unwrap();
        let out = run(&op, PREK);
        assert!(!out.contains("gitleaks"), "{out}");
        assert!(out.contains("make-check"), "{out}");
        assert!(out.parse::<DocumentMut>().is_ok(), "{out}");
    }

    #[test]
    fn a_missing_array_of_tables_element_is_created() {
        let op = KeyEdit::insert(
            "repos[repo=local].hooks[id=gitleaks]",
            "{ id = \"gitleaks\", entry = \"gitleaks git --staged\" }",
        )
        .unwrap();
        let out = run(&op, "# empty\n");
        let parsed: DocumentMut = out.parse().unwrap();
        let repos = parsed["repos"].as_array_of_tables().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos.get(0).unwrap()["repo"].as_str(), Some("local"));
    }

    #[test]
    fn array_of_tables_elements_are_selected_by_field() {
        let source = "[[bin]]\nname = \"a\"\npath = \"src/a.rs\"\n\n[[bin]]\nname = \"b\"\npath = \"src/b.rs\"\n";
        let op = KeyEdit::set("bin[name=b].path", "\"src/bin/b.rs\"").unwrap();
        assert_eq!(
            run(&op, source),
            "[[bin]]\nname = \"a\"\npath = \"src/a.rs\"\n\n[[bin]]\nname = \"b\"\npath = \"src/bin/b.rs\"\n"
        );
    }

    #[test]
    fn a_file_that_does_not_parse_is_left_alone() {
        let op = KeyEdit::set("a", "1").unwrap();
        assert_eq!(op.apply("a = [\n"), "a = [\n");
    }

    #[test]
    fn bad_values_and_self_unnamed_elements_are_rejected_when_built() {
        assert!(KeyEdit::set("a", "not toml at all").is_err());
        assert!(KeyEdit::set("a[id=x]", "{ id = \"y\" }").is_err());
        assert!(KeyEdit::set("a[id=x]", "3").is_err());
        assert!(KeyEdit::set("a..b", "1").is_err());
    }
}
