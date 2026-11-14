//! `markdown::frontmatter "<key>"`: set, insert or delete a top-level key
//! in a file's YAML front matter (the `---` block at its top), as skill
//! files (`name`, `description`) and wiki pages (`updated`, `status`) use.
//!
//! Only top-level keys are addressed. A key's entry is its line plus the
//! indented or `- ` lines under it, so a list or a block scalar is
//! replaced whole. Values are written as given, in YAML syntax, and
//! compared by meaning: `"x"`, `'x'` and `x` are equal, as are values that
//! differ only in spacing. Everything else in the front matter, and the
//! document after it, is left as it is.

use std::fmt;
use std::ops::Range;
use std::path::Path;

use colab_core::{Error, Operation, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Set(String),
    Insert(String),
    Delete,
}

/// One `markdown::frontmatter` rule.
#[derive(Debug)]
pub struct FrontmatterEdit {
    key: String,
    mode: Mode,
}

impl FrontmatterEdit {
    pub fn set(key: &str, value: &str) -> Result<Self> {
        Self::build(key, Mode::Set(value.trim_end().to_string()))
    }

    pub fn insert(key: &str, value: &str) -> Result<Self> {
        Self::build(key, Mode::Insert(value.trim_end().to_string()))
    }

    pub fn delete(key: &str) -> Result<Self> {
        Self::build(key, Mode::Delete)
    }

    fn build(key: &str, mode: Mode) -> Result<Self> {
        let valid = !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        if !valid {
            return Err(Error::Config(format!(
                "markdown::frontmatter \"{key}\": only top-level keys made of letters, digits, `-` and `_` are supported"
            )));
        }
        Ok(Self {
            key: key.to_string(),
            mode,
        })
    }

    fn entry(&self, value: &str) -> String {
        if value.contains('\n') {
            format!("{}:\n{}\n", self.key, value.trim_start_matches('\n'))
        } else {
            format!("{}: {}\n", self.key, value)
        }
    }
}

/// The front matter's body (between the `---` lines), or `None` when the
/// file has none.
fn block(source: &str) -> Option<Range<usize>> {
    let first = source.strip_prefix("---")?;
    let first_line_end = first.find('\n')?;
    if !first[..first_line_end].trim().is_empty() {
        return None;
    }
    let start = 3 + first_line_end + 1;
    let mut at = start;
    for line in source[start..].split_inclusive('\n') {
        let bare = line.trim_end();
        if bare == "---" || bare == "..." {
            return Some(start..at);
        }
        at += line.len();
    }
    None
}

/// The byte range of `key`'s entry inside the front matter body.
fn find_entry(source: &str, body: &Range<usize>, key: &str) -> Option<Range<usize>> {
    let mut at = body.start;
    let mut found: Option<usize> = None;
    for line in source[body.clone()].split_inclusive('\n') {
        let top_level = !line.starts_with([' ', '\t', '-', '#']) && !line.trim().is_empty();
        if let Some(start) = found {
            if top_level {
                return Some(start..at);
            }
        } else if top_level
            && line
                .split_once(':')
                .is_some_and(|(k, _)| k.trim().trim_matches(['"', '\'']) == key)
        {
            found = Some(at);
        }
        at += line.len();
    }
    found.map(|start| start..body.end)
}

/// A value as written after `key:` (and on the lines under it), reduced
/// to what it means for comparison.
fn meaning(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let unquoted = joined
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| joined.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(&joined);
    unquoted.to_string()
}

impl FrontmatterEdit {
    fn edit(&self, source: &str) -> Option<String> {
        let Some(body) = block(source) else {
            return match &self.mode {
                Mode::Delete => None,
                Mode::Set(v) | Mode::Insert(v) => {
                    Some(format!("---\n{}---\n\n{}", self.entry(v), source))
                }
            };
        };
        let entry = find_entry(source, &body, &self.key);
        let splice = |range: Range<usize>, with: &str| {
            format!("{}{with}{}", &source[..range.start], &source[range.end..])
        };
        match (&self.mode, entry) {
            (Mode::Delete, Some(range)) => Some(splice(range, "")),
            (Mode::Delete, None) | (Mode::Insert(_), Some(_)) => None,
            (Mode::Set(v) | Mode::Insert(v), None) => {
                Some(splice(body.end..body.end, &self.entry(v)))
            }
            (Mode::Set(v), Some(range)) => {
                let current = &source[range.clone()];
                let after_key = current.split_once(':').map_or("", |(_, rest)| rest);
                (meaning(after_key) != meaning(v)).then(|| splice(range, &self.entry(v)))
            }
        }
    }
}

impl fmt::Display for FrontmatterEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.mode {
            Mode::Set(v) => write!(f, "markdown::frontmatter \"{}\" = '{v}'", self.key),
            Mode::Insert(v) => write!(f, "markdown::frontmatter \"{}\" ?= '{v}'", self.key),
            Mode::Delete => write!(f, "markdown::frontmatter \"{}\" -> delete", self.key),
        }
    }
}

impl Operation for FrontmatterEdit {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_markdown(path)
    }

    fn apply(&self, source: &str) -> String {
        self.edit(source).unwrap_or_else(|| source.to_string())
    }

    /// `set` and `insert` act when the key is absent.
    fn prefilter(&self) -> Option<&str> {
        match self.mode {
            Mode::Delete => Some(&self.key),
            _ => None,
        }
    }

    fn near_miss_target(&self) -> Option<&str> {
        matches!(self.mode, Mode::Delete).then_some(self.key.as_str())
    }

    fn why_unchanged(&self, source: &str) -> Option<String> {
        (!matches!(self.mode, Mode::Delete) && self.edit(source).is_none())
            .then(|| crate::section::ALREADY.into())
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        let Some(body) = block(source) else {
            return Vec::new();
        };
        source[body]
            .lines()
            .filter(|l| !l.starts_with([' ', '\t', '-', '#']))
            .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim().to_string()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "---\ntitle: Plan\nupdated: 2026-11-01\ntags:\n  - a\n  - b\nstatus: draft\n---\n\n# Plan\n";

    #[test]
    fn set_replaces_by_meaning_and_keeps_the_rest() {
        let op = FrontmatterEdit::set("updated", "2026-11-13").unwrap();
        let out = op.apply(PAGE);
        assert_eq!(out, PAGE.replace("2026-11-01", "2026-11-13"));
        assert_eq!(op.apply(&out), out);
        assert_eq!(
            FrontmatterEdit::set("title", "\"Plan\"")
                .unwrap()
                .apply(PAGE),
            PAGE
        );
    }

    #[test]
    fn a_list_value_is_replaced_whole() {
        let op = FrontmatterEdit::set("tags", "\n  - c").unwrap();
        let out = op.apply(PAGE);
        assert!(out.contains("tags:\n  - c\nstatus: draft\n"), "{out}");
        assert_eq!(op.apply(&out), out);
    }

    #[test]
    fn insert_and_delete_and_a_file_without_front_matter() {
        let insert = FrontmatterEdit::insert("owner", "docs").unwrap();
        let out = insert.apply(PAGE);
        assert!(out.contains("status: draft\nowner: docs\n---\n"), "{out}");
        assert_eq!(insert.apply(&out), out);
        assert_eq!(
            FrontmatterEdit::insert("title", "Other")
                .unwrap()
                .apply(PAGE),
            PAGE
        );

        let out = FrontmatterEdit::delete("tags").unwrap().apply(PAGE);
        assert_eq!(
            out,
            "---\ntitle: Plan\nupdated: 2026-11-01\nstatus: draft\n---\n\n# Plan\n"
        );

        let bare = "# Doc\n";
        assert_eq!(
            FrontmatterEdit::set("status", "done").unwrap().apply(bare),
            "---\nstatus: done\n---\n\n# Doc\n"
        );
        assert_eq!(FrontmatterEdit::delete("status").unwrap().apply(bare), bare);
        assert!(FrontmatterEdit::set("a.b", "1").is_err());
    }
}
