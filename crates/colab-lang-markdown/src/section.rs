//! `markdown::section "<address>"`: set, insert, delete or rename a
//! section, addressed by heading path (`"Parent > Child"`) or anchor
//! (`"#slug"`). See [`crate::outline`] for what a section spans.

use std::fmt;
use std::path::Path;

use colab_core::{Error, Operation, Result};

use crate::outline::{self, Heading};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    /// Make everything under the heading exactly this text, adding the
    /// section (as `Insert` does) when it is missing.
    Set(String),
    /// Add the section, with this body, when it is missing.
    Insert(String),
    Delete,
    /// Give the heading this text.
    Rename(String),
}

/// Why a satisfied `set` / `insert` changed nothing: not a dead rule.
pub(crate) const ALREADY: &str = "already as the rule asks";

/// One `markdown::section` rule.
#[derive(Debug)]
pub struct SectionEdit {
    address: String,
    /// The longest word of the heading text, which survives inline markup
    /// and so must appear in any file whose section this rule changes.
    literal: Option<String>,
    mode: Mode,
}

impl SectionEdit {
    pub fn set(address: &str, body: &str) -> Result<Self> {
        Self::build(address, Mode::Set(normalise(body)))
    }

    pub fn insert(address: &str, body: &str) -> Result<Self> {
        if address.starts_with('#') {
            return Err(Error::Config(format!(
                "markdown::section \"{address}\": `insert` needs a heading path, which names the new heading; an anchor does not"
            )));
        }
        Self::build(address, Mode::Insert(normalise(body)))
    }

    pub fn delete(address: &str) -> Result<Self> {
        Self::build(address, Mode::Delete)
    }

    pub fn rename(address: &str, heading: &str) -> Result<Self> {
        let heading = heading.trim();
        if heading.is_empty() || heading.contains('\n') {
            return Err(Error::Config(format!(
                "markdown::section \"{address}\": the new heading must be one non-empty line"
            )));
        }
        Self::build(address, Mode::Rename(heading.to_string()))
    }

    fn build(address: &str, mode: Mode) -> Result<Self> {
        let address = address.trim().to_string();
        let literal = if let Some(slug) = address.strip_prefix('#') {
            if slug.is_empty() {
                return Err(Error::Config(
                    "markdown::section \"#\": an anchor needs a name".into(),
                ));
            }
            None
        } else {
            let segments = outline::segments(&address);
            let Some(last) = segments.last() else {
                return Err(Error::Config(
                    "markdown::section: the address names no heading".into(),
                ));
            };
            last.split_whitespace()
                .max_by_key(|w| w.len())
                .map(str::to_string)
        };
        Ok(Self {
            address,
            literal,
            mode,
        })
    }

    /// The rewritten source, or why this file is left alone.
    fn edit(&self, source: &str) -> std::result::Result<Option<String>, String> {
        let headings = outline::headings(source);
        if let Mode::Insert(body) = &self.mode {
            return self.insert_into(source, &headings, body);
        }
        let found = outline::find(&headings, &self.address);
        let section = match found.as_slice() {
            // `set` creates what is missing, as it does for every backend.
            [] => {
                return match &self.mode {
                    Mode::Set(body) if !self.address.starts_with('#') => {
                        self.insert_into(source, &headings, body)
                    }
                    _ => Ok(None),
                };
            }
            [one] => *one,
            _ => return Err(outline::resolve(&headings, &self.address).unwrap_err()),
        };
        Ok(match &self.mode {
            Mode::Set(body) => set_body(source, section, body),
            Mode::Delete => Some(delete(source, section)),
            Mode::Rename(text) => (source[section.content.clone()] != *text).then(|| {
                format!(
                    "{}{text}{}",
                    &source[..section.content.start],
                    &source[section.content.end..]
                )
            }),
            Mode::Insert(_) => unreachable!("handled above"),
        })
    }

    fn insert_into(
        &self,
        source: &str,
        headings: &[Heading],
        body: &str,
    ) -> std::result::Result<Option<String>, String> {
        if !outline::find(headings, &self.address).is_empty() {
            return Ok(None);
        }
        let mut segments = outline::segments(&self.address);
        segments.pop().expect("validated at build");
        // Written as given, markup included; matched as a reader sees it.
        let title = self
            .address
            .rsplit(" > ")
            .next()
            .unwrap_or(&self.address)
            .trim()
            .to_string();
        let (at, level) = if segments.is_empty() {
            // Top level: under a lone leading `#` title, else at the level
            // of the file's first heading.
            let level = match headings {
                [first, rest @ ..] if first.level == 1 && rest.iter().all(|h| h.level > 1) => 2,
                [first, ..] => first.level,
                [] => 1,
            };
            (source.len(), level)
        } else {
            let parent = outline::resolve(headings, &segments.join(" > "))
                .map_err(|e| format!("cannot insert `{}`: {e}", self.address))?;
            (parent.end, (parent.level + 1).min(6))
        };
        let before = &source[..at];
        let after = &source[at..];
        let mut out = String::with_capacity(source.len() + body.len() + title.len() + 8);
        out.push_str(before.trim_end_matches(['\n', ' ', '\t', '\r']));
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&"#".repeat(level as usize));
        out.push(' ');
        out.push_str(&title);
        out.push('\n');
        if !body.is_empty() {
            out.push('\n');
            out.push_str(body);
        }
        if after.trim().is_empty() {
            out.push_str(after.trim_start_matches(['\n', '\r']));
            if !out.ends_with('\n') {
                out.push('\n');
            }
        } else {
            out.push('\n');
            out.push_str(after.trim_start_matches(['\n', '\r']));
        }
        Ok(Some(out))
    }
}

/// Body text as whole lines with no surrounding blank lines, ending in a
/// newline (or empty).
fn normalise(body: &str) -> String {
    let lines: Vec<&str> = body.lines().map(str::trim_end).collect();
    let start = lines.iter().position(|l| !l.is_empty());
    let end = lines.iter().rposition(|l| !l.is_empty());
    match (start, end) {
        (Some(s), Some(e)) => lines[s..=e].join("\n") + "\n",
        _ => String::new(),
    }
}

/// Replace everything under the heading (subsections included) with
/// `body`, keeping one blank line after the heading and before what
/// follows. `None` when the body is already that, ignoring trailing
/// spaces and surrounding blank lines.
fn set_body(source: &str, section: &Heading, body: &str) -> Option<String> {
    let current = &source[section.heading_end..section.end];
    if normalise(current) == *body {
        return None;
    }
    let mut out = String::with_capacity(source.len() + body.len());
    out.push_str(&source[..section.heading_end]);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !body.is_empty() {
        out.push('\n');
        out.push_str(body);
    }
    let rest = &source[section.end..];
    if !rest.is_empty() {
        out.push('\n');
    }
    out.push_str(rest);
    Some(out)
}

/// Remove the section, heading through subsections, leaving one blank line
/// between what came before and after.
fn delete(source: &str, section: &Heading) -> String {
    let before = source[..section.start].trim_end_matches(['\n', '\r', ' ', '\t']);
    let after = &source[section.end..];
    match (before.is_empty(), after.is_empty()) {
        (true, _) => after.to_string(),
        (false, true) => format!("{before}\n"),
        (false, false) => format!("{before}\n\n{after}"),
    }
}

impl fmt::Display for SectionEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let action = match &self.mode {
            Mode::Set(_) => "set",
            Mode::Insert(_) => "insert",
            Mode::Delete => "delete",
            Mode::Rename(to) => {
                return write!(f, "markdown::section \"{}\" -> \"{to}\"", self.address);
            }
        };
        write!(f, "markdown::section \"{}\" -> {action}", self.address)
    }
}

impl Operation for SectionEdit {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_markdown(path)
    }

    fn apply(&self, source: &str) -> String {
        match self.edit(source) {
            Ok(Some(out)) => out,
            _ => source.to_string(),
        }
    }

    /// `set` and `insert` act when the section is missing, so they have none.
    fn prefilter(&self) -> Option<&str> {
        match self.mode {
            Mode::Set(_) | Mode::Insert(_) => None,
            _ => self.literal.as_deref(),
        }
    }

    fn near_miss_target(&self) -> Option<&str> {
        match self.mode {
            Mode::Set(_) | Mode::Insert(_) => None,
            _ => Some(&self.address),
        }
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        outline::headings(source)
            .iter()
            .map(|h| {
                if self.address.starts_with('#') {
                    format!("#{}", h.slug)
                } else {
                    h.address()
                }
            })
            .collect()
    }

    fn why_unchanged(&self, source: &str) -> Option<String> {
        match self.edit(source) {
            Err(reason) => Some(reason),
            Ok(None) if matches!(self.mode, Mode::Set(_) | Mode::Insert(_)) => Some(ALREADY.into()),
            Ok(_) => None,
        }
    }

    /// Renaming a heading changes its anchor; links in the same file to
    /// the old anchor would break.
    fn conflicts(&self, before: &str, _after: &str) -> Vec<String> {
        let Mode::Rename(to) = &self.mode else {
            return Vec::new();
        };
        let headings = outline::headings(before);
        let Ok(section) = outline::resolve(&headings, &self.address) else {
            return Vec::new();
        };
        let old = format!("](#{})", section.slug);
        let new_slug = outline::slug(&outline::plain_text(to));
        let count = before.matches(&old).count();
        if count == 0 || new_slug == section.slug {
            return Vec::new();
        }
        vec![format!(
            "{count} link(s) in this file point at `#{}`, which the rename makes `#{new_slug}`; add `match markdown::link \"#{}\" {{ replace \"#{new_slug}\" }}` once links are supported, or edit them",
            section.slug, section.slug
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str =
        "# Plan\n\n## Order\n\nold order\n\n### Detail\n\nmore\n\n## Risks\n\nrisky\n";

    #[test]
    fn set_replaces_the_body_and_is_idempotent() {
        let op = SectionEdit::set("Order", "new order\n\n- a").unwrap();
        let out = op.apply(DOC);
        assert_eq!(
            out,
            "# Plan\n\n## Order\n\nnew order\n\n- a\n\n## Risks\n\nrisky\n"
        );
        assert_eq!(op.apply(&out), out);
        // Ignores surrounding blank lines and trailing spaces.
        assert_eq!(
            SectionEdit::set("Risks", "risky  \n\n").unwrap().apply(DOC),
            DOC
        );
    }

    #[test]
    fn insert_adds_under_the_parent_once() {
        let op = SectionEdit::insert("Order > Notes", "a note").unwrap();
        let out = op.apply(DOC);
        assert_eq!(
            out,
            "# Plan\n\n## Order\n\nold order\n\n### Detail\n\nmore\n\n### Notes\n\na note\n\n## Risks\n\nrisky\n"
        );
        assert_eq!(op.apply(&out), out);
        let top = SectionEdit::insert("Non-goals", "none").unwrap().apply(DOC);
        assert!(
            top.ends_with("## Risks\n\nrisky\n\n## Non-goals\n\nnone\n"),
            "{top}"
        );
        let code = SectionEdit::insert("Order > `colab outline`", "x").unwrap();
        let out = code.apply(DOC);
        assert!(out.contains("### `colab outline`\n"), "{out}");
        assert_eq!(code.apply(&out), out, "found again by its plain text");
        let first = SectionEdit::insert("Intro", "hi").unwrap().apply("");
        assert_eq!(first, "# Intro\n\nhi\n");
    }

    #[test]
    fn delete_takes_subsections_and_keeps_spacing() {
        let out = SectionEdit::delete("Order").unwrap().apply(DOC);
        assert_eq!(out, "# Plan\n\n## Risks\n\nrisky\n");
        let last = SectionEdit::delete("Risks").unwrap().apply(DOC);
        assert!(last.ends_with("more\n"), "{last}");
    }

    #[test]
    fn rename_keeps_the_level_and_reports_links_to_the_old_anchor() {
        let doc = format!("{DOC}\nSee [order](#order).\n");
        let op = SectionEdit::rename("Order", "Order of work").unwrap();
        let out = op.apply(&doc);
        assert!(out.contains("## Order of work\n"));
        assert_eq!(
            op.apply(&out),
            out,
            "idempotent: the new heading no longer matches"
        );
        let conflicts = op.conflicts(&doc, &out);
        assert!(conflicts[0].contains("`#order`") && conflicts[0].contains("`#order-of-work`"));
    }

    #[test]
    fn an_ambiguous_address_changes_nothing_and_says_why() {
        let doc = "# A\n\n## X\n\none\n\n# B\n\n## X\n\ntwo\n";
        let op = SectionEdit::set("X", "new").unwrap();
        assert_eq!(op.apply(doc), doc);
        assert!(op.why_unchanged(doc).unwrap().contains("ambiguous"));
        assert_ne!(SectionEdit::set("B > X", "new").unwrap().apply(doc), doc);
        assert_eq!(SectionEdit::delete("X").unwrap().prefilter(), Some("X"));
        // `set` on a missing section adds it.
        let added = SectionEdit::set("B > Y", "three").unwrap().apply(doc);
        assert!(added.ends_with("## X\n\ntwo\n\n## Y\n\nthree\n"), "{added}");
    }
}
