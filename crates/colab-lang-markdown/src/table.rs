//! `markdown::row "<section>[<Col=value>, …]"`: set, insert or delete a
//! row of a pipe table in a section's own text.
//!
//! - `"Log[Date=2026-11-14]"`: rows of the section's first table whose
//!   `Date` cell is `2026-11-14`, compared as a reader sees them (no
//!   backticks or emphasis). Several `Col=value` pairs must all match.
//! - `"Log@2[…]"`: the section's second table.
//! - `"Log^[…]"`: an `insert` (or a `set` that adds) goes at the top,
//!   for newest-first tables.
//!
//! The value is either a whole row, `| a | b | c |`, or `Col=value` lines
//! naming some cells: `set` changes only those, and a new row leaves the
//! others empty. Cells are written with one space either side, and padded
//! to the column when the table is aligned; other rows are never touched.

use std::fmt;
use std::ops::Range;
use std::path::Path;

use colab_core::{Error, Operation, Result};

use crate::outline::{self, plain_text};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Set(Value),
    Insert(Value),
    Delete,
}

/// What a `set` or `insert` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    /// `| a | b |`: every cell, in order.
    Row(Vec<String>),
    /// `Col=value` lines: some cells, by column name.
    Cells(Vec<(String, String)>),
}

/// One `markdown::row` rule.
#[derive(Debug)]
pub struct RowEdit {
    /// The address in the form near misses compare (`outline::near_target`).
    near: String,
    target: String,
    section: String,
    nth: usize,
    top: bool,
    selector: Vec<(String, String)>,
    /// `"Log[…] < [Date=x]"`: a new row goes just before this row.
    before: Option<Vec<(String, String)>>,
    /// The longest selector value, which must appear in a file whose row
    /// this rule finds.
    mode: Mode,
}

/// A parsed pipe table: its cells' text, and where its lines are.
pub(crate) struct Table {
    pub(crate) header: Vec<String>,
    /// Each row's cells and the byte range of its line (newline included).
    pub(crate) rows: Vec<(Vec<String>, Range<usize>)>,
    /// Where a row goes at the top (after the delimiter line) and at the
    /// end (after the last row).
    top: usize,
    end: usize,
    /// Column widths when every existing line is padded to them.
    pub(crate) widths: Option<Vec<usize>>,
    /// Every line of the table, header and delimiter first, newline
    /// included.
    pub(crate) lines: Vec<Range<usize>>,
}

impl RowEdit {
    pub fn set(target: &str, value: &str) -> Result<Self> {
        Self::build(target, Some(value), |v| Mode::Set(v.expect("value")))
    }

    pub fn insert(target: &str, value: &str) -> Result<Self> {
        Self::build(target, Some(value), |v| Mode::Insert(v.expect("value")))
    }

    pub fn delete(target: &str) -> Result<Self> {
        Self::build(target, None, |_| Mode::Delete)
    }

    fn build(
        target: &str,
        value: Option<&str>,
        mode: impl FnOnce(Option<Value>) -> Mode,
    ) -> Result<Self> {
        let bad = |why: &str| Error::Config(format!("markdown::row \"{target}\": {why}"));
        let (main, before) = match target.split_once("] < [") {
            Some((row, sibling)) => (
                format!("{row}]"),
                Some(sibling.trim_end_matches(']').to_string()),
            ),
            None => (target.to_string(), None),
        };
        let before = before.map(|b| pairs(&b)).transpose().map_err(|e| bad(&e))?;
        if before.as_ref().is_some_and(std::vec::Vec::is_empty) {
            return Err(bad(
                "`< [ ]` needs the selector of the row to insert before",
            ));
        }
        let (section, nth, top, inner) = outline::split_target(&main).ok_or_else(|| {
            bad("give the section and a cell selector, e.g. \"Log[Date=2026-11-14]\"")
        })?;
        let selector = pairs(&inner).map_err(|e| bad(&e))?;
        let value = match value {
            None => None,
            Some(v) => Some(parse_value(v).map_err(|e| bad(&e))?),
        };
        if selector.is_empty() && !matches!(value, Some(Value::Row(_))) {
            return Err(bad(
                "an empty selector `[]` needs a whole row (`| a | b |`) as the value",
            ));
        }
        Ok(Self {
            near: outline::near_target(&section),
            target: target.trim().to_string(),
            section,
            nth: nth.unwrap_or(1),
            top,
            selector,
            before,
            mode: mode(value),
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
        let column = |name: &str| {
            table
                .header
                .iter()
                .position(|h| plain_text(h) == plain_text(name))
                .ok_or_else(|| {
                    format!(
                        "no column `{name}`; the columns are {}",
                        table
                            .header
                            .iter()
                            .map(|h| format!("`{}`", plain_text(h)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
        };
        let mut wanted: Vec<(usize, String)> = Vec::new();
        for (name, value) in &self.selector {
            wanted.push((column(name)?, plain_text(value)));
        }
        // An empty selector matches the row equal to the value.
        if wanted.is_empty()
            && let Mode::Set(Value::Row(cells)) | Mode::Insert(Value::Row(cells)) = &self.mode
        {
            wanted = cells.iter().map(|c| plain_text(c)).enumerate().collect();
        }
        let matches: Vec<usize> = table
            .rows
            .iter()
            .enumerate()
            .filter(|(_, (cells, _))| {
                wanted
                    .iter()
                    .all(|(i, v)| cells.get(*i).map(|c| plain_text(c)) == Some(v.clone()))
            })
            .map(|(i, _)| i)
            .collect();

        let row_for = |value: &Value,
                       base: Option<&Vec<String>>|
         -> std::result::Result<Vec<String>, String> {
            let mut cells = base
                .cloned()
                .unwrap_or_else(|| vec![String::new(); table.header.len()]);
            match value {
                Value::Row(all) => {
                    if all.len() != table.header.len() {
                        return Err(format!(
                            "the row has {} cells but the table has {} columns",
                            all.len(),
                            table.header.len()
                        ));
                    }
                    cells = all.clone();
                }
                Value::Cells(named) => {
                    for (name, v) in named {
                        cells[column(name)?] = v.clone();
                    }
                    // A new row also carries the selector's values, as
                    // written (markup included).
                    if base.is_none() {
                        for (name, v) in &self.selector {
                            let i = column(name)?;
                            if cells[i].is_empty() {
                                cells[i] = v.clone();
                            }
                        }
                    }
                }
            }
            Ok(cells)
        };
        let place = match &self.before {
            None => Ok(if self.top { table.top } else { table.end }),
            Some(sibling) => {
                let mut want = Vec::new();
                for (name, value) in sibling {
                    want.push((column(name)?, plain_text(value)));
                }
                let rows: Vec<&(Vec<String>, Range<usize>)> = table
                    .rows
                    .iter()
                    .filter(|(cells, _)| {
                        want.iter()
                            .all(|(i, v)| cells.get(*i).map(|c| plain_text(c)) == Some(v.clone()))
                    })
                    .collect();
                match rows.as_slice() {
                    [one] => Ok(one.1.start),
                    found => Err(format!(
                        "the row to insert before matches {} rows; it must match one",
                        found.len()
                    )),
                }
            }
        };
        let add = |cells: Vec<String>| -> std::result::Result<String, String> {
            let at = place.clone()?;
            let line = render(&cells, table.widths.as_deref());
            let mut out = String::with_capacity(source.len() + line.len() + 1);
            out.push_str(&source[..at]);
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&line);
            out.push('\n');
            out.push_str(&source[at..]);
            Ok(out)
        };

        Ok(match (&self.mode, matches.as_slice()) {
            (Mode::Delete, []) | (Mode::Insert(_), [_, ..]) => None,
            (Mode::Delete, rows) => {
                let mut out = source.to_string();
                for &i in rows.iter().rev() {
                    out.replace_range(table.rows[i].1.clone(), "");
                }
                Some(out)
            }
            // A `set` that changes a selector column renames the row; once
            // done, the row matching the new values is the same row.
            (Mode::Set(Value::Cells(named)), []) if self.renamed(named, &table, &column)? => None,
            (Mode::Set(v) | Mode::Insert(v), []) => Some(add(row_for(v, None)?)?),
            (Mode::Set(v), [i]) => {
                let (current, range) = &table.rows[*i];
                let cells = row_for(v, Some(current))?;
                let same = cells
                    .iter()
                    .map(|c| plain_text(c))
                    .eq(current.iter().map(|c| plain_text(c)));
                (!same).then(|| {
                    let line = render(&cells, table.widths.as_deref());
                    let mut out = source.to_string();
                    let end = if source[range.clone()].ends_with('\n') {
                        range.end - 1
                    } else {
                        range.end
                    };
                    out.replace_range(range.start..end, &line);
                    out
                })
            }
            (Mode::Set(_), many) => {
                return Err(format!(
                    "`{}` matches {} rows; a `set` needs exactly one (narrow the selector)",
                    self.target,
                    many.len()
                ));
            }
        })
    }
}

impl RowEdit {
    /// `true` when `named` changes a selector column and a row already
    /// matches the selector with those new values: the rename is done.
    fn renamed(
        &self,
        named: &[(String, String)],
        table: &Table,
        column: &dyn Fn(&str) -> std::result::Result<usize, String>,
    ) -> std::result::Result<bool, String> {
        let mut changes_selector = false;
        let mut want: Vec<(usize, String)> = Vec::new();
        for (name, value) in &self.selector {
            let new = named
                .iter()
                .find(|(n, _)| plain_text(n) == plain_text(name))
                .map(|(_, v)| v);
            changes_selector |= new.is_some();
            want.push((column(name)?, plain_text(new.unwrap_or(value))));
        }
        Ok(changes_selector
            && table.rows.iter().any(|(cells, _)| {
                want.iter()
                    .all(|(i, v)| cells.get(*i).map(|c| plain_text(c)) == Some(v.clone()))
            }))
    }
}

/// `Col=value, Col=value` (a selector) into pairs. A value may contain
/// `=`; it is split at the first.
fn pairs(text: &str) -> std::result::Result<Vec<(String, String)>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    // A comma separates pairs only when `Column=` follows it, so a value
    // may itself contain commas.
    let mut parts: Vec<String> = Vec::new();
    for part in split_top_level(text, ',') {
        match parts.last_mut() {
            Some(last) if !part.contains('=') => {
                last.push(',');
                last.push_str(&part);
            }
            _ => parts.push(part),
        }
    }
    parts
        .into_iter()
        .map(|part| {
            part.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .filter(|(k, _)| !k.is_empty())
                .ok_or_else(|| format!("`{}` is not `Column=value`", part.trim()))
        })
        .collect()
}

/// Split on `sep` outside backticks, so a value like `` `a, b` `` stays whole.
fn split_top_level(text: &str, sep: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut in_code = false;
    for c in text.chars() {
        if c == '`' {
            in_code = !in_code;
        }
        if c == sep && !in_code {
            parts.push(String::new());
        } else {
            parts.last_mut().expect("never empty").push(c);
        }
    }
    parts
}

fn parse_value(text: &str) -> std::result::Result<Value, String> {
    let trimmed = text.trim();
    if trimmed.starts_with('|') {
        if trimmed.contains('\n') {
            return Err("a row value is one `| … |` line".into());
        }
        return Ok(Value::Row(split_row(trimmed)));
    }
    let cells: std::result::Result<Vec<_>, String> = trimmed
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            line.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .ok_or_else(|| {
                    format!(
                        "`{}` is neither a `| … |` row nor `Column=value`",
                        line.trim()
                    )
                })
        })
        .collect();
    let cells = cells?;
    if cells.is_empty() {
        return Err("the value names no cells".into());
    }
    Ok(Value::Cells(cells))
}

/// The cells of a `| a | b |` line, trimmed. As in GFM, only `\|`
/// stays inside a cell, even within backticks.
pub(crate) fn split_row(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|');
    let inner = inner
        .strip_suffix('|')
        .filter(|i| !i.ends_with('\\'))
        .unwrap_or(inner);
    let mut cells = vec![String::new()];
    let mut prev = '\0';
    for c in inner.chars() {
        if c == '|' && prev != '\\' {
            cells.push(String::new());
        } else {
            cells.last_mut().expect("never empty").push(c);
        }
        prev = c;
    }
    cells.into_iter().map(|c| c.trim().to_string()).collect()
}

fn render(cells: &[String], widths: Option<&[usize]>) -> String {
    let mut line = String::from("|");
    for (i, cell) in cells.iter().enumerate() {
        let cell = escape_pipes(cell);
        let width = widths.and_then(|w| w.get(i)).copied().unwrap_or(0);
        let pad = width.saturating_sub(cell.chars().count());
        line.push(' ');
        line.push_str(&cell);
        line.push_str(&" ".repeat(pad));
        line.push_str(" |");
    }
    line
}

/// `|` in a value is escaped unless it already is; GFM needs the escape
/// inside backticks too.
fn escape_pipes(cell: &str) -> String {
    let mut out = String::with_capacity(cell.len());
    let mut prev = '\0';
    for c in cell.chars() {
        if c == '|' && prev != '\\' {
            out.push('\\');
        }
        out.push(c);
        prev = c;
    }
    out
}

/// The `nth` (1-based) pipe table starting inside `own`.
pub(crate) fn nth_table(source: &str, own: Range<usize>, nth: usize) -> Option<Table> {
    let tree = crate::parse(source)?;
    let tables = crate::nodes(&tree, |n| {
        n.kind() == "pipe_table" && own.contains(&n.start_byte())
    });
    let node = *tables.get(nth.checked_sub(1)?)?;
    let line_of = |n: tree_sitter::Node<'_>| {
        let start = source[..n.start_byte()].rfind('\n').map_or(0, |i| i + 1);
        let end = source[n.start_byte()..]
            .find('\n')
            .map_or(source.len(), |i| n.start_byte() + i + 1);
        start..end
    };
    let mut header = Vec::new();
    let mut rows = Vec::new();
    let mut top = 0;
    let mut end = 0;
    let mut lines: Vec<Range<usize>> = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let range = line_of(child);
        match child.kind() {
            "pipe_table_header" => {
                header = split_row(source[range.clone()].trim_end());
                lines.push(range);
            }
            "pipe_table_delimiter_row" => {
                top = range.end;
                end = range.end;
                lines.push(range);
            }
            "pipe_table_row" => {
                rows.push((split_row(source[range.clone()].trim_end()), range.clone()));
                end = range.end;
                lines.push(range);
            }
            _ => {}
        }
    }
    let widths = aligned_widths(source, &lines);
    Some(Table {
        header,
        rows,
        top,
        end,
        widths,
        lines,
    })
}

/// When every line of the table has its `|` at the same columns, the
/// cell widths that keeps them there; otherwise `None`.
fn aligned_widths(source: &str, lines: &[Range<usize>]) -> Option<Vec<usize>> {
    let pipes = |r: &Range<usize>| -> Vec<usize> {
        source[r.clone()]
            .trim_end()
            .char_indices()
            .filter(|(_, c)| *c == '|')
            .map(|(i, _)| source[r.start..r.start + i].chars().count())
            .collect()
    };
    let first = pipes(lines.first()?);
    if lines.len() < 2 || lines.iter().any(|l| pipes(l) != first) {
        return None;
    }
    Some(first.windows(2).map(|w| w[1] - w[0] - 3).collect())
}

impl fmt::Display for RowEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let action = match &self.mode {
            Mode::Set(_) => "set",
            Mode::Insert(_) => "insert",
            Mode::Delete => "delete",
        };
        write!(f, "markdown::row \"{}\" -> {action}", self.target)
    }
}

impl Operation for RowEdit {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_markdown(path)
    }

    fn apply(&self, source: &str) -> String {
        match self.edit(source) {
            Ok(Some(out)) => out,
            _ => source.to_string(),
        }
    }

    /// `set` and `insert` act when the row is missing.
    // None: cells are compared as a reader sees them, and no word of that
    // text is sure to appear in the file (`PQ, RS` is `` `PQ`, `RS` `` there).
    // Markdown parses cheaply.
    fn prefilter(&self) -> Option<&str> {
        None
    }

    /// The section named by the address: a rule whose section exists
    /// nowhere lists the nearest headings.
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

    const LOG: &str = "# Log\n\n| Date | Task | Outcome |\n| ---- | ---- | ------- |\n| 2026-11-13 | a | open |\n| 2026-11-12 | b | done |\n\n## Later\n\n| Date | X |\n| - | - |\n| 2026-11-13 | sub |\n";

    #[test]
    fn a_selector_beside_markup_still_reaches_the_file() {
        // `PQ, RS` as a reader sees it is `` `PQ`, `RS` `` in the file: no
        // word of the selector need appear there, so nothing may be
        // prefiltered on it (it was, and the row was silently skipped).
        let src = "# T\n\n| Item | N |\n| - | - |\n| `PQ`, `RS` | 4 |\n| other | 5 |\n";
        let op = RowEdit::delete("T[Item=PQ, RS]").unwrap();
        let out = op.apply(src);
        assert_eq!(out, "# T\n\n| Item | N |\n| - | - |\n| other | 5 |\n");
        assert!(op.prefilter().is_none_or(|p| src.contains(p)));
    }

    #[test]
    fn set_changes_only_the_named_cells_of_one_row() {
        let op = RowEdit::set("Log[Date=2026-11-13]", "Outcome=Fixed in `abc`").unwrap();
        let out = op.apply(LOG);
        assert!(
            out.contains("| 2026-11-13 | a | Fixed in `abc` |\n| 2026-11-12"),
            "{out}"
        );
        assert!(
            out.contains("| 2026-11-13 | sub |"),
            "the subsection's table is untouched"
        );
        assert_eq!(op.apply(&out), out);
    }

    #[test]
    fn insert_adds_at_the_end_or_top_once() {
        let end = RowEdit::insert("Log[Date=2026-11-14]", "Task=c\nOutcome=new").unwrap();
        let out = end.apply(LOG);
        assert!(
            out.contains("| 2026-11-12 | b | done |\n| 2026-11-14 | c | new |\n\n## Later"),
            "{out}"
        );
        assert_eq!(end.apply(&out), out);
        let top = RowEdit::insert("Log^[Date=2026-11-14]", "| 2026-11-14 | c | new |").unwrap();
        let out = top.apply(LOG);
        assert!(
            out.contains("| ------- |\n| 2026-11-14 | c | new |\n| 2026-11-13"),
            "{out}"
        );
    }

    #[test]
    fn a_new_rows_selector_cells_keep_their_markup() {
        let op = RowEdit::insert("Log[Task=`raw` strings]", "Outcome=done").unwrap();
        let out = op.apply(LOG);
        assert!(out.contains("|  | `raw` strings | done |"), "{out}");
        // Matched by the text a reader sees, so a delete without the
        // backticks finds it; with no prefilter, no file is skipped.
        let delete = RowEdit::delete("Log[Task=raw strings]").unwrap();
        assert_eq!(delete.prefilter(), None);
        assert!(!delete.apply(&out).contains("raw"));
    }

    #[test]
    fn a_selector_value_may_contain_commas() {
        let pairs = pairs("Date=2026-11-14, Task=docs (5 scripts, 30 rules)").unwrap();
        assert_eq!(
            pairs[1],
            ("Task".into(), "docs (5 scripts, 30 rules)".into())
        );
    }

    #[test]
    fn a_set_that_renames_its_selector_is_idempotent() {
        let op = RowEdit::set("Log[Task=a]", "Task=a, renamed").unwrap();
        let out = op.apply(LOG);
        assert!(out.contains("| 2026-11-13 | a, renamed | open |"), "{out}");
        assert_eq!(op.apply(&out), out, "the renamed row is not added again");
    }

    #[test]
    fn insert_before_a_row() {
        let op = RowEdit::insert("Log[Date=2026-11-12.5] < [Date=2026-11-12]", "Task=mid").unwrap();
        let out = op.apply(LOG);
        assert!(
            out.contains("| open |\n| 2026-11-12.5 | mid |  |\n| 2026-11-12 |"),
            "{out}"
        );
        assert_eq!(op.apply(&out), out);
        let none = RowEdit::insert("Log[Date=x] < [Date=nope]", "Task=y").unwrap();
        assert!(none.why_unchanged(LOG).unwrap().contains("matches 0 rows"));
    }

    #[test]
    fn delete_removes_every_matching_row() {
        let out = RowEdit::delete("Log[Outcome=done]").unwrap().apply(LOG);
        assert!(!out.contains("2026-11-12"));
        assert!(out.contains("| 2026-11-13 | a | open |"));
    }

    #[test]
    fn mistakes_change_nothing_and_say_why() {
        let op = RowEdit::set("Log[Day=2026-11-13]", "Outcome=x").unwrap();
        assert_eq!(op.apply(LOG), LOG);
        assert!(
            op.why_unchanged(LOG)
                .unwrap()
                .contains("the columns are `Date`, `Task`, `Outcome`")
        );
        let wide = RowEdit::set("Log[Date=2026-11-13]", "| a | b |").unwrap();
        assert!(
            wide.why_unchanged(LOG)
                .unwrap()
                .contains("2 cells but the table has 3")
        );
        assert!(RowEdit::delete("Log").is_err());
        assert!(
            RowEdit::set("Log@2[Date=x]", "X=1")
                .unwrap()
                .why_unchanged(LOG)
                .unwrap()
                .contains("no table 2")
        );
    }

    #[test]
    fn an_aligned_table_stays_aligned() {
        let doc = "## T\n\n| Name | N |\n| ---- | - |\n| a    | 1 |\n";
        let out = RowEdit::insert("T[Name=b]", "N=2").unwrap().apply(doc);
        assert!(out.ends_with("| a    | 1 |\n| b    | 2 |\n"), "{out}");
        assert_eq!(split_row(r"| a \| b | `c\|d` |"), [r"a \| b", r"`c\|d`"]);
    }
}
