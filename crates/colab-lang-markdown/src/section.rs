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
pub(crate) const ALREADY: &str = colab_core::ALREADY;

/// One `markdown::section` rule.
#[derive(Debug)]
pub struct SectionEdit {
    /// The address in the form near misses compare (`outline::near_target`).
    near: String,
    address: String,
    /// `"New < Sibling"`: an insert goes just before `Sibling`, at its
    /// level, instead of at the end of the parent.
    before: Option<String>,
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
        let (address, before) = match address.rsplit_once(" < ") {
            Some((new, sibling)) => (new.trim().to_string(), Some(sibling.trim().to_string())),
            None => (address.trim().to_string(), None),
        };
        if before.is_some() && !matches!(mode, Mode::Set(_) | Mode::Insert(_)) {
            return Err(Error::Config(format!(
                "markdown::section \"{address} < …\": `< Sibling` places a new section, so it goes with `set` or `insert` only"
            )));
        }
        if let Some(slug) = address.strip_prefix('#') {
            if slug.is_empty() {
                return Err(Error::Config(
                    "markdown::section \"#\": an anchor needs a name".into(),
                ));
            }
        } else if outline::segments(&address).is_empty() {
            return Err(Error::Config(
                "markdown::section: the address names no heading".into(),
            ));
        }
        Ok(Self {
            near: outline::near_target(&address),
            address,
            before,
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
        let (at, level) = if let Some(sibling) = &self.before {
            let sibling = outline::resolve(headings, sibling)
                .map_err(|e| format!("cannot insert `{}` before it: {e}", self.address))?;
            (sibling.start, sibling.level)
        } else if segments.is_empty() {
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

    /// None: `set` and `insert` act when the section is missing, and a
    /// heading is matched on the text a reader sees, of which no word is
    /// sure to appear in the file as written (`` Use `fix`, then ``).
    fn prefilter(&self) -> Option<&str> {
        None
    }

    fn near_miss_target(&self) -> Option<&str> {
        match self.mode {
            Mode::Set(_) | Mode::Insert(_) => None,
            _ => Some(&self.near),
        }
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        outline::observed_addresses(source, &self.address)
    }

    fn why_unchanged(&self, source: &str) -> Option<String> {
        match self.edit(source) {
            Err(reason) => Some(reason),
            Ok(None) if matches!(self.mode, Mode::Set(_) | Mode::Insert(_)) => Some(ALREADY.into()),
            Ok(_) => None,
        }
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
    fn insert_before_a_sibling_takes_its_place_and_level() {
        let op = SectionEdit::insert("Scope < Risks", "in scope").unwrap();
        let out = op.apply(DOC);
        assert!(
            out.contains("more\n\n## Scope\n\nin scope\n\n## Risks\n"),
            "{out}"
        );
        assert_eq!(op.apply(&out), out);
        assert!(SectionEdit::delete("A < B").is_err());
    }

    #[test]
    fn delete_takes_subsections_and_keeps_spacing() {
        let out = SectionEdit::delete("Order").unwrap().apply(DOC);
        assert_eq!(out, "# Plan\n\n## Risks\n\nrisky\n");
        let last = SectionEdit::delete("Risks").unwrap().apply(DOC);
        assert!(last.ends_with("more\n"), "{last}");
    }

    #[test]
    fn rename_keeps_the_level() {
        let doc = format!("{DOC}\nSee [order](#order).\n");
        let op = SectionEdit::rename("Order", "Order of work").unwrap();
        let out = op.apply(&doc);
        assert!(out.contains("## Order of work\n"));
        assert_eq!(
            op.apply(&out),
            out,
            "idempotent: the new heading no longer matches"
        );
    }

    #[test]
    fn an_ambiguous_address_changes_nothing_and_says_why() {
        let doc = "# A\n\n## X\n\none\n\n# B\n\n## X\n\ntwo\n";
        let op = SectionEdit::set("X", "new").unwrap();
        assert_eq!(op.apply(doc), doc);
        assert!(op.why_unchanged(doc).unwrap().contains("ambiguous"));
        assert_ne!(SectionEdit::set("B > X", "new").unwrap().apply(doc), doc);
        // Headings are matched as a reader sees them: no prefilter.
        assert_eq!(SectionEdit::delete("X").unwrap().prefilter(), None);
        // `set` on a missing section adds it.
        let added = SectionEdit::set("B > Y", "three").unwrap().apply(doc);
        assert!(added.ends_with("## X\n\ntwo\n\n## Y\n\nthree\n"), "{added}");
    }
}
