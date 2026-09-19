//! `markdown::block`: set, insert or delete a managed block.

use std::fmt;
use std::path::Path;

use colab_core::{Error, Operation, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Set(String),
    Insert(String),
    Delete,
}

/// One `markdown::block` rule.
#[derive(Debug)]
pub struct BlockEdit {
    name: String,
    mode: Mode,
}

impl BlockEdit {
    pub fn set(name: &str, content: &str) -> Result<Self> {
        Self::build(name, Mode::Set(normalise(content)))
    }

    pub fn insert(name: &str, content: &str) -> Result<Self> {
        Self::build(name, Mode::Insert(normalise(content)))
    }

    pub fn delete(name: &str) -> Result<Self> {
        Self::build(name, Mode::Delete)
    }

    fn build(name: &str, mode: Mode) -> Result<Self> {
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !valid {
            return Err(Error::Config(format!(
                "markdown::block \"{name}\": a block name may use only letters, digits, `-`, `_` and `.`"
            )));
        }
        Ok(Self {
            name: name.to_string(),
            mode,
        })
    }

    fn begin(&self) -> String {
        format!("<!-- {}:begin", self.name)
    }

    fn end(&self) -> String {
        format!("<!-- {}:end -->", self.name)
    }
}

/// Content as whole lines: no leading or trailing blank lines, each line
/// ending in `\n`, and empty for empty content.
fn normalise(content: &str) -> String {
    let trimmed = content.trim_matches(|c| c == '\n' || c == '\r');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{}\n", trimmed.replace("\r\n", "\n"))
    }
}

impl fmt::Display for BlockEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let action = match self.mode {
            Mode::Set(_) => "set",
            Mode::Insert(_) => "insert",
            Mode::Delete => "delete",
        };
        write!(f, "markdown::block \"{}\" -> {action}", self.name)
    }
}

/// Where the block sits: the indexes of its begin and end marker lines.
enum Found {
    Absent,
    At {
        begin: usize,
        end: usize,
    },
    /// Markers repeated, out of order, or unbalanced.
    Malformed,
}

impl Operation for BlockEdit {
    fn is_file_relevant(&self, path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("md" | "markdown")
        )
    }

    fn apply(&self, source_code: &str) -> String {
        let lines: Vec<&str> = source_code.split_inclusive('\n').collect();
        let (begin_marker, end_marker) = (self.begin(), self.end());
        let is_begin = |line: &str| {
            let line = line.trim();
            line.starts_with(&begin_marker)
                && line.ends_with("-->")
                && line[begin_marker.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_whitespace() || c == '-')
        };
        let is_end = |line: &str| line.trim() == end_marker;
        let begins: Vec<usize> = (0..lines.len()).filter(|&i| is_begin(lines[i])).collect();
        let ends: Vec<usize> = (0..lines.len()).filter(|&i| is_end(lines[i])).collect();
        let found = match (begins.as_slice(), ends.as_slice()) {
            ([], []) => Found::Absent,
            ([begin], [end]) if begin < end => Found::At {
                begin: *begin,
                end: *end,
            },
            _ => Found::Malformed,
        };

        match (&self.mode, found) {
            (_, Found::Malformed)
            | (Mode::Delete, Found::Absent)
            | (Mode::Insert(_), Found::At { .. }) => source_code.to_string(),
            (Mode::Set(content) | Mode::Insert(content), Found::Absent) => {
                let mut out = source_code.to_string();
                if !out.is_empty() {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    if !out.ends_with("\n\n") {
                        out.push('\n');
                    }
                }
                out.push_str(&format!(
                    "<!-- {}:begin -->\n{content}{end_marker}\n",
                    self.name
                ));
                out
            }
            (Mode::Set(content), Found::At { begin, end }) => {
                let current: String = lines[begin + 1..end].concat().replace("\r\n", "\n");
                if current == *content {
                    return source_code.to_string();
                }
                let mut out = lines[..=begin].concat();
                out.push_str(content);
                out.push_str(&lines[end..].concat());
                out
            }
            (Mode::Delete, Found::At { begin, end }) => {
                let mut after = end + 1;
                // Take one blank line with the block when it would otherwise
                // leave two in a row, or one at the start of the file.
                let blank_before = begin == 0 || lines[begin - 1].trim().is_empty();
                if blank_before && lines.get(after).is_some_and(|l| l.trim().is_empty()) {
                    after += 1;
                }
                let mut out = lines[..begin].concat();
                out.push_str(&lines[after..].concat());
                out
            }
        }
    }

    fn prefilter(&self) -> Option<&str> {
        // Only `delete` needs the block to exist; `set` and `insert` act
        // when it is absent.
        match self.mode {
            Mode::Delete => Some(&self.name),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENTS: &str = "# Agents\n\nIntro written by hand.\n\n<!-- myspec:begin — managed by myspec (profile rust) -->\n## myspec\n\n- `myspec report`\n<!-- myspec:end -->\n\nMore by hand.\n";

    fn run(op: &BlockEdit, source: &str) -> String {
        let once = op.apply(source);
        assert_eq!(op.apply(&once), once, "not idempotent: {op}");
        once
    }

    #[test]
    fn set_replaces_only_the_inside_and_keeps_the_begin_line() {
        let op = BlockEdit::set(
            "myspec",
            "## myspec\n\n- `myspec report`\n- `myspec tools status`\n",
        )
        .unwrap();
        let out = run(&op, AGENTS);
        assert_eq!(
            out,
            "# Agents\n\nIntro written by hand.\n\n<!-- myspec:begin — managed by myspec (profile rust) -->\n## myspec\n\n- `myspec report`\n- `myspec tools status`\n<!-- myspec:end -->\n\nMore by hand.\n"
        );
    }

    #[test]
    fn identical_content_is_a_no_op() {
        let op = BlockEdit::set("myspec", "\n## myspec\n\n- `myspec report`").unwrap();
        assert_eq!(op.apply(AGENTS), AGENTS);
    }

    #[test]
    fn a_missing_block_is_appended_after_a_blank_line() {
        let op = BlockEdit::set("tool", "managed").unwrap();
        assert_eq!(
            run(&op, "# Title\nText"),
            "# Title\nText\n\n<!-- tool:begin -->\nmanaged\n<!-- tool:end -->\n"
        );
        assert_eq!(
            run(&op, ""),
            "<!-- tool:begin -->\nmanaged\n<!-- tool:end -->\n"
        );
    }

    #[test]
    fn insert_leaves_an_existing_block_alone() {
        let op = BlockEdit::insert("myspec", "replacement").unwrap();
        assert_eq!(op.apply(AGENTS), AGENTS);
    }

    #[test]
    fn delete_removes_the_block_and_one_blank_line() {
        let op = BlockEdit::delete("myspec").unwrap();
        assert_eq!(
            run(&op, AGENTS),
            "# Agents\n\nIntro written by hand.\n\nMore by hand.\n"
        );
    }

    #[test]
    fn another_blocks_name_is_not_this_block() {
        let op = BlockEdit::set("my", "x").unwrap();
        let out = op.apply(AGENTS);
        assert!(out.contains("<!-- myspec:begin"), "{out}");
        assert!(
            out.ends_with("<!-- my:begin -->\nx\n<!-- my:end -->\n"),
            "{out}"
        );
    }

    #[test]
    fn repeated_or_unbalanced_markers_are_left_alone() {
        let op = BlockEdit::set("tool", "x").unwrap();
        for source in [
            "<!-- tool:begin -->\na\n<!-- tool:end -->\n<!-- tool:begin -->\nb\n<!-- tool:end -->\n",
            "<!-- tool:begin -->\na\n",
            "<!-- tool:end -->\n<!-- tool:begin -->\n",
        ] {
            assert_eq!(op.apply(source), source);
        }
    }

    #[test]
    fn bad_names_are_rejected() {
        assert!(BlockEdit::set("a b", "x").is_err());
        assert!(BlockEdit::delete("").is_err());
        assert!(BlockEdit::delete("a-->").is_err());
    }
}
