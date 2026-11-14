//! `markdown::text "<section>[<text>]"`: replace or delete a phrase in a
//! section's own text, for edits smaller than a section, row or item.
//!
//! The text is matched word for word, with any run of whitespace in it
//! matching any run in the file, so a phrase wrapped across lines still
//! matches. Code fences are skipped. Every occurrence in the section's own
//! text (before its first subsection) changes; `delete` also removes one
//! space beside the phrase so no double space is left.

use std::fmt;
use std::ops::Range;
use std::path::Path;

use colab_core::{Error, Operation, Result};

use crate::outline;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Replace(String),
    Delete,
}

/// One `markdown::text` rule.
#[derive(Debug)]
pub struct TextEdit {
    target: String,
    section: String,
    words: Vec<String>,
    mode: Mode,
}

impl TextEdit {
    pub fn replace(target: &str, with: &str) -> Result<Self> {
        Self::build(target, Mode::Replace(with.to_string()))
    }

    pub fn delete(target: &str) -> Result<Self> {
        Self::build(target, Mode::Delete)
    }

    fn build(target: &str, mode: Mode) -> Result<Self> {
        let (section, _, _, inner) = outline::split_target(target).ok_or_else(|| {
            Error::Config(format!(
                "markdown::text \"{target}\": give the section and the text, e.g. \"Order of work[M28 has shipped.]\""
            ))
        })?;
        let words: Vec<String> = inner.split_whitespace().map(str::to_string).collect();
        if words.is_empty() {
            return Err(Error::Config(format!(
                "markdown::text \"{target}\": the text in brackets is empty"
            )));
        }
        Ok(Self {
            target: target.trim().to_string(),
            section,
            words,
            mode,
        })
    }

    /// Every occurrence of the words in `hay`, as byte ranges.
    fn occurrences(&self, hay: &str) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let first = &self.words[0];
        let mut from = 0;
        while let Some(i) = hay[from..].find(first.as_str()) {
            let start = from + i;
            let mut at = start + first.len();
            let mut ok = true;
            for word in &self.words[1..] {
                let gap = hay[at..].len() - hay[at..].trim_start().len();
                if gap == 0 || !hay[at + gap..].starts_with(word.as_str()) {
                    ok = false;
                    break;
                }
                at += gap + word.len();
            }
            if ok {
                out.push(start..at);
                from = at;
            } else {
                from = start + first.len();
            }
        }
        out
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
        // Fences inside the section are code, not text.
        let fences: Vec<Range<usize>> = crate::fences(source)
            .into_iter()
            .map(|f| f.content)
            .filter(|r| own.contains(&r.start))
            .collect();
        let mut hits: Vec<Range<usize>> = self
            .occurrences(&source[own.clone()])
            .into_iter()
            .map(|r| own.start + r.start..own.start + r.end)
            .filter(|r| !fences.iter().any(|f| f.start <= r.start && r.end <= f.end))
            .collect();
        // A replacement that contains the phrase (`[a]` → `[a] [b]`) would
        // match again on the next run: an occurrence already standing where
        // the replacement puts it is done.
        if let Mode::Replace(with) = &self.mode {
            let inner = self.occurrences(with);
            hits.retain(|hit| {
                !inner.iter().any(|k| {
                    hit.start >= k.start && source[hit.start - k.start..].starts_with(with.as_str())
                })
            });
        }
        if hits.is_empty() {
            return Ok(None);
        }
        let mut out = source.to_string();
        hits.reverse();
        for hit in hits {
            match &self.mode {
                Mode::Replace(with) => out.replace_range(hit, with),
                Mode::Delete => {
                    // Take one space with it: the one after, else before.
                    let range = if out[hit.end..].starts_with(' ') {
                        hit.start..hit.end + 1
                    } else if out[..hit.start].ends_with(' ') {
                        hit.start - 1..hit.end
                    } else {
                        hit
                    };
                    out.replace_range(range, "");
                }
            }
        }
        Ok((out != source).then_some(out))
    }
}

impl fmt::Display for TextEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.mode {
            Mode::Replace(to) => write!(f, "markdown::text \"{}\" -> \"{to}\"", self.target),
            Mode::Delete => write!(f, "markdown::text \"{}\" -> delete", self.target),
        }
    }
}

impl Operation for TextEdit {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_markdown(path)
    }

    fn apply(&self, source: &str) -> String {
        match self.edit(source) {
            Ok(Some(out)) => out,
            _ => source.to_string(),
        }
    }

    fn prefilter(&self) -> Option<&str> {
        self.words
            .iter()
            .max_by_key(|w| w.len())
            .map(String::as_str)
    }

    fn why_unchanged(&self, source: &str) -> Option<String> {
        self.edit(source).err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "## Order\n\n**M28 → M23 → M20**, then M14. M28 has\nshipped.\n\n```sh\nM28 has shipped.\n```\n\n### Detail\n\nM28 has shipped.\n";

    #[test]
    fn a_wrapped_phrase_matches_and_fences_and_subsections_are_left() {
        let op = TextEdit::replace("Order[M28 has shipped.]", "M28 is done.").unwrap();
        let out = op.apply(DOC);
        assert!(out.contains("then M14. M28 is done.\n"), "{out}");
        assert!(
            out.contains("```sh\nM28 has shipped.\n```"),
            "code is not text"
        );
        assert!(
            out.ends_with("### Detail\n\nM28 has shipped.\n"),
            "the subsection is not own text"
        );
        assert_eq!(op.apply(&out), out);
    }

    #[test]
    fn a_replacement_containing_the_phrase_is_idempotent() {
        let op = TextEdit::replace("Order[then M14.]", "then M14. Then M21.").unwrap();
        let out = op.apply(DOC);
        assert!(out.contains("then M14. Then M21. M28"), "{out}");
        assert_eq!(op.apply(&out), out);
    }

    #[test]
    fn delete_takes_one_space_with_it() {
        let out = TextEdit::delete("Order[M28 →]").unwrap().apply(DOC);
        assert!(out.contains("**M23 → M20**"), "{out}");
    }
}
