//! `markdown::item "<section>[<text>]"`: ensure, delete or replace one
//! item of a list in a section's own text.
//!
//! An item is named by the start of its text as a reader sees it, so
//! `"Unscheduled[Call and type hierarchy]"` names the bullet that begins
//! "Call and type hierarchy (…), the last …". `@N` picks the section's
//! Nth list. Only top-level items of that list are addressed; an item's
//! nested list goes with it.
//!
//! - `ensure` adds `- <text>` (in the list's own marker style) when no
//!   item starts with the text, and creates the list when the section has
//!   none.
//! - `delete` removes the item, nested lines included.
//! - `replace "<text>"` gives the item new text, keeping its marker and
//!   nested list.

use std::fmt;
use std::ops::Range;
use std::path::Path;

use colab_core::{Error, Operation, Result};

use crate::outline::{self, plain_text};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Ensure,
    Delete,
    Replace(String),
}

/// One `markdown::item` rule.
#[derive(Debug)]
pub struct ItemEdit {
    /// The address in the form near misses compare (`outline::near_target`).
    near: String,
    target: String,
    section: String,
    nth: usize,
    /// The item text as written (an `ensure` writes it).
    text: String,
    /// Where an `ensure` puts a new item: before the item starting with
    /// this text (`"Notes[New] < [Old]"`), or before the first (`^`).
    /// `None`: after the last.
    before: Option<Before>,
    mode: Mode,
}

#[derive(Debug)]
enum Before {
    First,
    Item(String),
}

/// One top-level item: its marker, where its first paragraph's text is,
/// and the item's whole span (nested lines included).
struct Item {
    marker: String,
    text: Range<usize>,
    span: Range<usize>,
}

impl ItemEdit {
    pub fn ensure(target: &str) -> Result<Self> {
        Self::build(target, Mode::Ensure)
    }

    pub fn delete(target: &str) -> Result<Self> {
        Self::build(target, Mode::Delete)
    }

    pub fn replace(target: &str, text: &str) -> Result<Self> {
        Self::build(target, Mode::Replace(text.trim().to_string()))
    }

    fn build(target: &str, mode: Mode) -> Result<Self> {
        let (main, before_text) = match target.split_once("] < [") {
            Some((main, rest)) => (
                format!("{main}]"),
                Some(rest.strip_suffix(']').unwrap_or(rest).trim().to_string()),
            ),
            None => (target.to_string(), None),
        };
        let (section, nth, top, text) = outline::split_target(&main).ok_or_else(|| {
            Error::Config(format!(
                "markdown::item \"{target}\": give the section and the item's text, e.g. \"Notes[Raw strings]\""
            ))
        })?;
        if text.trim().is_empty() {
            return Err(Error::Config(format!(
                "markdown::item \"{target}\": the item text in brackets is empty"
            )));
        }
        let before = match (before_text, top) {
            (Some(b), _) if b.is_empty() => {
                return Err(Error::Config(format!(
                    "markdown::item \"{target}\": `< [ ]` needs the start of the item to insert before"
                )));
            }
            (Some(b), _) => Some(Before::Item(b)),
            (None, true) => Some(Before::First),
            (None, false) => None,
        };
        if before.is_some() && !matches!(mode, Mode::Ensure) {
            return Err(Error::Config(format!(
                "markdown::item \"{target}\": `< [ … ]` and `^` place a new item, so they go with `ensure` only"
            )));
        }
        Ok(Self {
            before,
            near: outline::near_target(&section),
            target: target.trim().to_string(),
            section,
            nth: nth.unwrap_or(1),
            text: text.trim().to_string(),
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
        let items = nth_list(source, own.clone(), self.nth);
        let want = plain_text(&self.text);
        let matching: Vec<&Item> = items
            .iter()
            .filter(|i| plain_text(&source[i.text.clone()]).starts_with(&want))
            .collect();
        if matching.len() > 1 && !matches!(self.mode, Mode::Ensure) {
            return Err(format!(
                "{} items of `{}` start with `{want}`; give more of the text",
                matching.len(),
                section.address()
            ));
        }
        Ok(match (&self.mode, matching.first()) {
            (Mode::Ensure, Some(_)) | (Mode::Delete | Mode::Replace(_), None) => None,
            (Mode::Ensure, None) if self.before.is_some() && !items.is_empty() => {
                let anchor = match self.before.as_ref() {
                    Some(Before::Item(start)) => {
                        let start = plain_text(start);
                        let found: Vec<&Item> = items
                            .iter()
                            .filter(|i| plain_text(&source[i.text.clone()]).starts_with(&start))
                            .collect();
                        match found.as_slice() {
                            [one] => *one,
                            [] => {
                                return Err(format!(
                                    "no item of `{}` starts with `{start}` to insert before",
                                    section.address()
                                ));
                            }
                            many => {
                                return Err(format!(
                                    "the item to insert before matches {} items; give more of its text",
                                    many.len()
                                ));
                            }
                        }
                    }
                    _ => &items[0],
                };
                // The neighbour's marker and indent; an ordered list keeps
                // its numbers, which renderers renumber anyway.
                let at = line_start(source, anchor.span.start);
                let indent = &source[at..anchor.span.start];
                let marker = &anchor.marker;
                let width = wrap_width(source, &items);
                let hang = format!("{indent}{}", " ".repeat(marker.chars().count()));
                let mut out = source[..at].to_string();
                out.push_str(&format!(
                    "{indent}{marker}{}\n",
                    wrap(&self.text, width, indent.len() + marker.len(), &hang)
                ));
                out.push_str(&source[at..]);
                Some(out)
            }
            (Mode::Ensure, None) => Some(match items.last() {
                Some(last) => {
                    let marker = next_marker(&last.marker);
                    let indent = &source[line_start(source, last.span.start)..last.span.start];
                    let at = last.span.end;
                    let mut out = source[..at].to_string();
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    let width = wrap_width(source, &items);
                    let hang = format!("{indent}{}", " ".repeat(marker.chars().count()));
                    out.push_str(&format!(
                        "{indent}{marker}{}\n",
                        wrap(&self.text, width, indent.len() + marker.len(), &hang)
                    ));
                    out.push_str(&source[at..]);
                    out
                }
                None => {
                    // No list yet: one bullet at the end of the section's own text.
                    let end = own.end;
                    let before = source[..end].trim_end_matches(['\n', ' ', '\t', '\r']);
                    let rest = &source[end..];
                    let sep = if rest.is_empty() { "\n" } else { "\n\n" };
                    format!("{before}\n\n- {}{sep}{rest}", self.text)
                }
            }),
            (Mode::Delete, Some(item)) => {
                let mut out = source.to_string();
                let start = line_start(source, item.span.start);
                out.replace_range(start..item.span.end, "");
                Some(out)
            }
            (Mode::Replace(text), Some(item)) => (source[item.text.clone()] != **text).then(|| {
                format!(
                    "{}{text}{}",
                    &source[..item.text.start],
                    &source[item.text.end..]
                )
            }),
        })
    }
}

/// The width to wrap a new item at: when any existing item of the list
/// spans more than one line, the longest line in the list; otherwise
/// `None`, and the item stays on one line, like its neighbours.
fn wrap_width(source: &str, items: &[Item]) -> Option<usize> {
    // An item's own text across lines, not a nested list under it.
    let wrapped = items
        .iter()
        .any(|i| source[i.text.clone()].trim_end().contains('\n'));
    wrapped.then(|| {
        items
            .iter()
            .flat_map(|i| source[i.span.clone()].lines())
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(72)
    })
}

/// `text` broken into lines of at most `width` characters (the first
/// already holding `first` characters of marker), later lines starting
/// with `hang`. Backtick spans are never broken.
fn wrap(text: &str, width: Option<usize>, first: usize, hang: &str) -> String {
    let Some(width) = width else {
        return text.to_string();
    };
    // Words, with a backtick span kept as one.
    let mut words: Vec<String> = Vec::new();
    let mut in_code = false;
    for word in text.split(' ') {
        match words.last_mut() {
            Some(last) if in_code => {
                last.push(' ');
                last.push_str(word);
            }
            _ => words.push(word.to_string()),
        }
        if word.matches('`').count() % 2 == 1 {
            in_code = !in_code;
        }
    }
    let mut out = String::new();
    let mut line_len = first;
    for word in words.into_iter().filter(|w| !w.is_empty()) {
        let len = word.chars().count();
        if line_len > first.min(line_len) && line_len + 1 + len > width && !out.is_empty() {
            out.push('\n');
            out.push_str(hang);
            line_len = hang.chars().count();
        } else if !out.is_empty() {
            out.push(' ');
            line_len += 1;
        }
        out.push_str(&word);
        line_len += len;
    }
    out
}

/// The marker a new item after one with `marker` gets: the same bullet,
/// or the next number.
fn next_marker(marker: &str) -> String {
    let digits: String = marker.chars().take_while(|c| c.is_ascii_digit()).collect();
    match digits.parse::<u64>() {
        Ok(n) => format!("{}{}", n + 1, &marker[digits.len()..]),
        Err(_) => marker.to_string(),
    }
}

fn line_start(source: &str, at: usize) -> usize {
    source[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// The top-level items of the `nth` (1-based) outermost list starting in
/// `own`.
fn nth_list(source: &str, own: Range<usize>, nth: usize) -> Vec<Item> {
    let Some(tree) = crate::parse(source) else {
        return Vec::new();
    };
    let lists = crate::nodes(&tree, |n| {
        n.kind() == "list"
            && own.contains(&n.start_byte())
            && !std::iter::successors(n.parent(), |p| p.parent()).any(|p| p.kind() == "list")
    });
    let Some(list) = nth.checked_sub(1).and_then(|i| lists.get(i)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = list.walk();
    for item in list.children(&mut cursor) {
        if item.kind() != "list_item" {
            continue;
        }
        let mut c = item.walk();
        let children: Vec<_> = item.children(&mut c).collect();
        let Some(marker) = children
            .iter()
            .find(|n| n.kind().starts_with("list_marker"))
        else {
            continue;
        };
        let text = children
            .iter()
            .find(|n| n.kind() == "paragraph")
            .and_then(|p| {
                let mut pc = p.walk();
                p.children(&mut pc)
                    .find(|n| n.kind() == "inline")
                    .map(|n| n.byte_range())
            })
            .unwrap_or(marker.end_byte()..marker.end_byte());
        // The item's span ends at its last line's end, never in the blank
        // line after it.
        let end = source[..item.end_byte()]
            .trim_end_matches(['\n', '\r', ' ', '\t'])
            .len();
        let end = source[end..]
            .find('\n')
            .map_or(source.len(), |i| end + i + 1);
        out.push(Item {
            marker: source[marker.byte_range()].to_string(),
            text,
            span: item.start_byte()..end,
        });
    }
    out
}

impl fmt::Display for ItemEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.mode {
            Mode::Ensure => write!(f, "markdown::item \"{}\" -> ensure", self.target),
            Mode::Delete => write!(f, "markdown::item \"{}\" -> delete", self.target),
            Mode::Replace(t) => write!(f, "markdown::item \"{}\" -> \"{t}\"", self.target),
        }
    }
}

impl Operation for ItemEdit {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_markdown(path)
    }

    fn apply(&self, source: &str) -> String {
        match self.edit(source) {
            Ok(Some(out)) => out,
            _ => source.to_string(),
        }
    }

    /// None: an item is named by the text a reader sees, and no word of
    /// it is sure to appear in the file as written (see `RowEdit`).
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
            Ok(None) if matches!(self.mode, Mode::Ensure) => Some(crate::section::ALREADY.into()),
            Ok(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "## Notes\n\n- Raw strings in the DSL\n- Tab re-indent\n  - nested detail\n- Call and type hierarchy\n\n## Steps\n\n1. one\n2. two\n";

    #[test]
    fn ensure_places_a_new_item_before_another_or_first() {
        let op = ItemEdit::ensure("Notes[Kept servers] < [Tab re-indent]").unwrap();
        let out = op.apply(DOC);
        assert!(
            out.contains(
                "- Raw strings in the DSL\n- Kept servers\n- Tab re-indent\n  - nested detail\n"
            ),
            "{out}"
        );
        assert_eq!(op.apply(&out), out, "idempotent");
        let top = ItemEdit::ensure("Steps^[zero]").unwrap().apply(DOC);
        assert!(
            top.ends_with("## Steps\n\n1. zero\n1. one\n2. two\n"),
            "{top}"
        );
        let missing = ItemEdit::ensure("Notes[x] < [Nope]").unwrap();
        assert!(
            missing
                .why_unchanged(DOC)
                .unwrap()
                .contains("no item of `Notes` starts with `Nope`")
        );
        assert!(ItemEdit::delete("Notes[x] < [Tab]").is_err());
    }

    #[test]
    fn ensure_adds_once_in_the_lists_style() {
        let op = ItemEdit::ensure("Notes[Shorthand fields]").unwrap();
        let out = op.apply(DOC);
        assert!(
            out.contains("- Call and type hierarchy\n- Shorthand fields\n\n## Steps"),
            "{out}"
        );
        assert_eq!(op.apply(&out), out);
        let numbered = ItemEdit::ensure("Steps[three]").unwrap().apply(DOC);
        assert!(numbered.ends_with("2. two\n3. three\n"), "{numbered}");
        let fresh = ItemEdit::ensure("Steps > Later[first]").unwrap();
        assert_eq!(fresh.apply(DOC), DOC, "a missing section is not created");
        let empty = ItemEdit::ensure("A[x]")
            .unwrap()
            .apply("## A\n\nText.\n\n## B\n");
        assert_eq!(empty, "## A\n\nText.\n\n- x\n\n## B\n");
    }

    #[test]
    fn ensure_wraps_like_the_lists_wrapped_items() {
        let doc = "## W\n\n- short\n- a longer item that wraps onto a\n  second line here\n";
        let op =
            ItemEdit::ensure("W[a new item with `code that stays whole` and enough words to wrap]")
                .unwrap();
        let out = op.apply(doc);
        let added = out.split("here\n").nth(1).unwrap();
        assert!(added.lines().count() > 1, "{out}");
        assert!(added.lines().all(|l| l.chars().count() <= 34), "{out}");
        assert!(added.contains("`code that stays whole`"), "{out}");
        assert_eq!(op.apply(&out), out);
        // Unwrapped neighbours: one line.
        let flat = ItemEdit::ensure("Notes[a long item that would otherwise wrap past seventy-two columns if wrapping applied]").unwrap();
        let out = flat.apply(DOC);
        assert!(out.contains(
            "- a long item that would otherwise wrap past seventy-two columns if wrapping applied\n"
        ));
    }

    #[test]
    fn delete_takes_nested_lines_and_replace_keeps_them() {
        let out = ItemEdit::delete("Notes[Tab]").unwrap().apply(DOC);
        assert_eq!(out, DOC.replace("- Tab re-indent\n  - nested detail\n", ""));
        let op = ItemEdit::replace("Notes[Tab]", "Tab re-indent (fixed)").unwrap();
        let out = op.apply(DOC);
        assert!(
            out.contains("- Tab re-indent (fixed)\n  - nested detail\n"),
            "{out}"
        );
    }

    #[test]
    fn an_ambiguous_prefix_changes_nothing() {
        let doc = "## N\n\n- Raw a\n- Raw b\n";
        let op = ItemEdit::delete("N[Raw]").unwrap();
        assert_eq!(op.apply(doc), doc);
        assert!(op.why_unchanged(doc).unwrap().contains("2 items"));
    }
}
