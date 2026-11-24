//! `markdown::column "<section>[<Header>]"`: insert, delete or rename a
//! column of a pipe table in a section's own text.
//!
//! - `"Matrix[create]"`: the column headed `create` in the section's first
//!   table, compared as a reader sees it (no backticks or emphasis).
//!   `@2` picks the second table.
//! - `insert '<cell>'` adds the column with that cell in every row, last,
//!   or first with `^` (`"Matrix^[create]"`), or before another column
//!   with `< [Header]` (`"Matrix[create] < [Applies to]"`).
//! - `delete` removes it; `replace '<header>'` renames it.
//!
//! Each line is spliced at its pipes, so the other cells keep their
//! spacing; in an aligned table the new cells are padded to one width.
//! Give single rows their values with `markdown::row … { set 'Col=…' }`.

use std::fmt;
use std::ops::Range;
use std::path::Path;

use colab_core::{Error, Operation, Result};

use crate::outline::{self, plain_text};
use crate::table::{Table, nth_table, split_row};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Insert(String),
    Delete,
    Rename(String),
}

/// One `markdown::column` rule.
#[derive(Debug)]
pub struct ColumnEdit {
    /// The address in the form near misses compare (`outline::near_target`).
    near: String,
    target: String,
    section: String,
    nth: usize,
    /// `^`: a new column goes first.
    first: bool,
    header: String,
    /// `< [Header]`: a new column goes just before this one.
    before: Option<String>,
    mode: Mode,
}

impl ColumnEdit {
    pub fn insert(target: &str, cell: &str) -> Result<Self> {
        Self::build(target, Mode::Insert(cell.trim().to_string()))
    }

    pub fn delete(target: &str) -> Result<Self> {
        Self::build(target, Mode::Delete)
    }

    pub fn rename(target: &str, header: &str) -> Result<Self> {
        Self::build(target, Mode::Rename(header.trim().to_string()))
    }

    fn build(target: &str, mode: Mode) -> Result<Self> {
        let bad = |why: &str| Error::Config(format!("markdown::column \"{target}\": {why}"));
        let (main, before) = match target.split_once("] < [") {
            Some((column, sibling)) => (
                format!("{column}]"),
                Some(sibling.trim_end_matches(']').trim().to_string()),
            ),
            None => (target.to_string(), None),
        };
        if before.is_some() && !matches!(mode, Mode::Insert(_)) {
            return Err(bad(
                "`< [Header]` places a new column, so only `insert` takes it",
            ));
        }
        let (section, nth, first, header) = outline::split_target(&main)
            .ok_or_else(|| bad("give the section and a column header, e.g. \"Matrix[create]\""))?;
        let header = header.trim().to_string();
        if header.is_empty() || before.as_deref() == Some("") {
            return Err(bad("name the column by its header"));
        }
        if matches!(&mode, Mode::Rename(new) if new.is_empty()) {
            return Err(bad("`replace` needs the new header"));
        }
        Ok(Self {
            near: outline::near_target(&section),
            target: target.trim().to_string(),
            section,
            nth: nth.unwrap_or(1),
            first,
            header,
            before,
            mode,
        })
    }

    fn edit(&self, source: &str) -> std::result::Result<Option<String>, String> {
        let headings = outline::headings(source);
        let found = outline::find(&headings, &self.section);
        let section = match found.as_slice() {
            [] => return Ok(None),
            [one] => *one,
            _ => return Err(outline::resolve(&headings, &self.section).unwrap_err()),
        };
        let own = section.heading_end..outline::own_end(&headings, section);
        let Some(table) = nth_table(source, own, self.nth) else {
            return Err(format!(
                "`{}` has no table {} in its own text (before its first subsection)",
                section.address(),
                self.nth
            ));
        };
        if let Some(line) = table
            .lines
            .iter()
            .find(|l| !source[(*l).clone()].trim_start().starts_with('|'))
        {
            return Err(format!(
                "the table's line `{}` does not start with `|`; columns are edited only in \
                 tables whose lines do",
                source[line.clone()].trim_end()
            ));
        }
        let position = |name: &str| {
            table
                .header
                .iter()
                .position(|h| plain_text(h) == plain_text(name))
        };
        let columns = || {
            table
                .header
                .iter()
                .map(|h| format!("`{}`", plain_text(h)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match &self.mode {
            Mode::Insert(cell) => {
                if position(&self.header).is_some() {
                    return Ok(None);
                }
                let at = match &self.before {
                    Some(name) => position(name).ok_or_else(|| {
                        format!(
                            "no column `{name}` to insert before; the columns are {}",
                            columns()
                        )
                    })?,
                    None if self.first => 0,
                    None => table.header.len(),
                };
                Ok(Some(insert_column(source, &table, at, &self.header, cell)))
            }
            Mode::Delete => Ok(position(&self.header).map(|at| delete_column(source, &table, at))),
            Mode::Rename(new) => match (position(&self.header), position(new)) {
                (Some(at), _) => Ok(Some(rename_column(source, &table, at, new))),
                // Done already: the new header is there and the old is not.
                (None, Some(_)) => Ok(None),
                (None, None) => Err(format!(
                    "no column `{}`; the columns are {}",
                    self.header,
                    columns()
                )),
            },
        }
    }
}

/// The byte offsets of a line's unescaped `|`s, relative to the line.
fn pipes(line: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut prev = '\0';
    for (i, c) in line.char_indices() {
        if c == '|' && prev != '\\' {
            out.push(i);
        }
        prev = c;
    }
    out
}

/// `source` with each table line rewritten by `edit(line index, line
/// text without its newline)`, bottom up so earlier offsets hold.
fn rewrite_lines(source: &str, table: &Table, edit: impl Fn(usize, &str) -> String) -> String {
    let mut out = source.to_string();
    for (i, range) in table.lines.iter().enumerate().rev() {
        let text = &source[range.clone()];
        let body = text.trim_end_matches(['\n', '\r']);
        let body_range: Range<usize> = range.start..range.start + body.len();
        out.replace_range(body_range, &edit(i, body));
    }
    out
}

fn insert_column(source: &str, table: &Table, at: usize, header: &str, cell: &str) -> String {
    // An aligned table stays aligned: the new cells share one width.
    let width = table.widths.as_ref().map(|_| {
        [header.chars().count(), cell.chars().count(), 3]
            .into_iter()
            .max()
            .unwrap_or(3)
    });
    let pad = |text: &str| match width {
        Some(w) => format!(
            "{text}{}",
            " ".repeat(w.saturating_sub(text.chars().count()))
        ),
        None => text.to_string(),
    };
    rewrite_lines(source, table, |i, line| {
        let value = match i {
            0 => pad(header),
            1 => "-".repeat(width.unwrap_or(3)),
            _ => pad(cell),
        };
        let pipes = pipes(line);
        // Before cell `at`: just after the pipe that opens it. After the
        // last cell: after the closing pipe, or appended when there is none.
        let cells = split_row(line).len();
        let closed = cells < pipes.len();
        match pipes.get(at) {
            Some(&p) if at < cells => format!("{} {value} |{}", &line[..=p], &line[p + 1..]),
            _ if closed => format!("{line} {value} |"),
            _ => format!("{line} | {value}"),
        }
    })
}

fn delete_column(source: &str, table: &Table, at: usize) -> String {
    rewrite_lines(source, table, |_, line| {
        let pipes = pipes(line);
        match (pipes.get(at), pipes.get(at + 1)) {
            // From the pipe that opens the cell to the next one.
            (Some(&open), Some(&next)) => format!("{}{}", &line[..open], &line[next..]),
            // The last cell of a row with no closing pipe.
            (Some(&open), None) => line[..open].trim_end().to_string(),
            _ => line.to_string(),
        }
    })
}

fn rename_column(source: &str, table: &Table, at: usize, new: &str) -> String {
    rewrite_lines(source, table, |i, line| {
        if i != 0 {
            return line.to_string();
        }
        let pipes = pipes(line);
        let (Some(&open), close) = (pipes.get(at), pipes.get(at + 1).copied()) else {
            return line.to_string();
        };
        let close = close.unwrap_or(line.len());
        let old = &line[open + 1..close];
        // Keep the cell's width when the new header fits in it.
        let width = old.chars().count().saturating_sub(2);
        let padded = format!(
            "{new}{}",
            " ".repeat(width.saturating_sub(new.chars().count()))
        );
        let tail = if close < line.len() { " " } else { "" };
        format!("{} {padded}{tail}{}", &line[..=open], &line[close..])
    })
}

impl fmt::Display for ColumnEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let action = match &self.mode {
            Mode::Insert(_) => "insert".to_string(),
            Mode::Delete => "delete".to_string(),
            Mode::Rename(new) => format!("'{new}'"),
        };
        write!(f, "markdown::column \"{}\" -> {action}", self.target)
    }
}

impl Operation for ColumnEdit {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_markdown(path)
    }

    fn apply(&self, source: &str) -> String {
        match self.edit(source) {
            Ok(Some(out)) => out,
            _ => source.to_string(),
        }
    }

    /// `insert` acts when the column is missing; headers are compared as
    /// a reader sees them, so no literal is sure to be in the file.
    fn prefilter(&self) -> Option<&str> {
        None
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.near)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        crate::outline::observed_addresses(source, &self.section)
    }

    fn why_unchanged(&self, source: &str) -> Option<String> {
        match self.edit(source) {
            Err(reason) => Some(reason),
            Ok(None) if !matches!(self.mode, Mode::Delete) => Some(crate::section::ALREADY.into()),
            Ok(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# M\n\n| Name | `a` | Files |\n| ---- | :-: | ----- |\n| x | ✅ | `.rs` |\n| y |    | `.go` |\n\n## Sub\n\n| Name | B |\n| - | - |\n";

    #[test]
    fn insert_adds_a_column_before_another_once() {
        let op = ColumnEdit::insert("M[`b`] < [Files]", "").unwrap();
        let out = op.apply(DOC);
        assert!(
            out.contains("| Name | `a` | `b` | Files |\n| ---- | :-: | --- | ----- |\n| x | ✅ |  | `.rs` |\n| y |    |  | `.go` |\n"),
            "{out}"
        );
        assert!(
            out.ends_with("| Name | B |\n| - | - |\n"),
            "the subsection's table is untouched"
        );
        assert_eq!(op.apply(&out), out);
    }

    #[test]
    fn insert_at_the_end_and_first() {
        let end = ColumnEdit::insert("M[N]", "0").unwrap().apply(DOC);
        assert!(
            end.contains(
                "| Name | `a` | Files | N |\n| ---- | :-: | ----- | --- |\n| x | ✅ | `.rs` | 0 |"
            ),
            "{end}"
        );
        let first = ColumnEdit::insert("M^[#]", "1").unwrap().apply(DOC);
        assert!(
            first.contains("| # | Name | `a` | Files |\n| --- | ---- |"),
            "{first}"
        );
    }

    #[test]
    fn delete_and_rename() {
        let out = ColumnEdit::delete("M[a]").unwrap().apply(DOC);
        assert!(
            out.contains("| Name | Files |\n| ---- | ----- |\n| x | `.rs` |\n| y | `.go` |\n"),
            "{out}"
        );
        let op = ColumnEdit::rename("M[Files]", "Applies to").unwrap();
        let out = op.apply(DOC);
        assert!(out.contains("| Name | `a` | Applies to |\n"), "{out}");
        assert_eq!(op.apply(&out), out);
    }

    #[test]
    fn an_aligned_table_stays_aligned() {
        let doc = "## T\n\n| Name | N |\n| ---- | - |\n| a    | 1 |\n";
        let out = ColumnEdit::insert("T[Kind] < [N]", "x").unwrap().apply(doc);
        assert_eq!(
            out,
            "## T\n\n| Name | Kind | N |\n| ---- | ---- | - |\n| a    | x    | 1 |\n"
        );
    }

    #[test]
    fn mistakes_change_nothing_and_say_why() {
        let op = ColumnEdit::insert("M[z] < [Nope]", "").unwrap();
        assert_eq!(op.apply(DOC), DOC);
        assert!(
            op.why_unchanged(DOC)
                .unwrap()
                .contains("the columns are `Name`, `a`, `Files`")
        );
        assert!(ColumnEdit::delete("M[z] < [a]").is_err());
        assert!(ColumnEdit::rename("M[Name]", " ").is_err());
    }
}
