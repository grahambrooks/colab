//! `text::literal` and `text::word`: a literal string, replaced or deleted
//! wherever it occurs.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use colab_core::find::Located;
use colab_core::{Error, Operation, Result};

/// How an occurrence of the target is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every occurrence.
    Literal,
    /// Only an occurrence that is not part of a longer word.
    Word,
}

impl Mode {
    fn module(self) -> &'static str {
        match self {
            Mode::Literal => "literal",
            Mode::Word => "word",
        }
    }
}

/// Extensions of formats that are not text. A text rule skips them rather
/// than reading each one to find it is not UTF-8, which the walk would
/// list as a skipped file: noise on every run over a tree with images.
/// A binary file with any other extension is still skipped, just listed.
const BINARY: &[&str] = &[
    "7z", "a", "avif", "bin", "bmp", "class", "dll", "dylib", "eot", "exe", "gif", "gz", "ico",
    "jar", "jpeg", "jpg", "lib", "mov", "mp3", "mp4", "o", "obj", "otf", "pdf", "png", "pyc", "so",
    "tar", "tgz", "ttf", "wasm", "wav", "webm", "webp", "woff", "woff2", "xz", "zip", "zst",
];

/// Replace (or, with no replacement, delete) a literal string.
#[derive(Debug)]
pub struct Literal {
    mode: Mode,
    target: String,
    replacement: Option<String>,
}

impl Literal {
    pub fn new(mode: Mode, target: String, replacement: Option<String>) -> Result<Self> {
        let problem =
            |text: &str| Error::Config(format!("text::{} \"{target}\": {text}", mode.module()));
        if target.is_empty() {
            return Err(problem("the target is empty, so it would match everywhere"));
        }
        if replacement.as_deref() == Some(target.as_str()) {
            return Err(problem(
                "the replacement is the target itself, so it changes nothing",
            ));
        }
        Ok(Self {
            mode,
            target,
            replacement,
        })
    }

    /// Byte ranges of the accepted occurrences, in order, not overlapping.
    fn occurrences(&self, source: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(offset) = source[from..].find(&self.target) {
            let start = from + offset;
            let end = start + self.target.len();
            if self.accepts(source, start, end) {
                out.push((start, end));
                from = end;
            } else {
                // Step one character, so an accepted occurrence that
                // overlaps a rejected one is still found.
                from = start + source[start..].chars().next().map_or(1, char::len_utf8);
            }
        }
        out
    }

    fn accepts(&self, source: &str, start: usize, end: usize) -> bool {
        match self.mode {
            Mode::Literal => true,
            Mode::Word => bounded(source, start, end),
        }
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `true` when `source[start..end]` is not part of a longer word: an edge
/// that is a word character has no word character beside it.
fn bounded(source: &str, start: usize, end: usize) -> bool {
    let text = &source[start..end];
    let first = text.chars().next().is_some_and(is_word);
    let last = text.chars().next_back().is_some_and(is_word);
    let before = source[..start].chars().next_back().is_some_and(is_word);
    let after = source[end..].chars().next().is_some_and(is_word);
    !(first && before) && !(last && after)
}

/// The word around `source[start..end]`: the range widened over word
/// characters on either side.
fn enclosing_word(source: &str, start: usize, end: usize) -> &str {
    let start = source[..start]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(start, |(i, _)| i);
    let end = end
        + source[end..]
            .char_indices()
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
    &source[start..end]
}

impl Operation for Literal {
    fn is_file_relevant(&self, path: &Path) -> bool {
        let ext = path.extension().and_then(|e| e.to_str());
        !ext.is_some_and(|e| BINARY.iter().any(|b| b.eq_ignore_ascii_case(e)))
    }

    fn apply(&self, source: &str) -> String {
        let found = self.occurrences(source);
        if found.is_empty() {
            return source.to_string();
        }
        let with = self.replacement.as_deref().unwrap_or("");
        let mut out = String::with_capacity(source.len());
        let mut last = 0;
        for (start, end) in found {
            out.push_str(&source[last..start]);
            out.push_str(with);
            last = end;
        }
        out.push_str(&source[last..]);
        out
    }

    /// The target itself: no occurrence can be accepted where it is absent.
    fn prefilter(&self) -> Option<&str> {
        Some(&self.target)
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.target)
    }

    /// What is nearly the target: the same text in another case, and, for
    /// `word`, the longer words the target occurs inside.
    fn observed_targets(&self, source: &str) -> Vec<String> {
        let mut seen = BTreeSet::new();
        // ASCII lowering keeps byte offsets, so a hit in the lowered text
        // is a range of the original.
        let lowered = source.to_ascii_lowercase();
        let needle = self.target.to_ascii_lowercase();
        for (start, _) in lowered.match_indices(&needle) {
            let end = start + needle.len();
            if !source.is_char_boundary(start) || !source.is_char_boundary(end) {
                continue;
            }
            let text = &source[start..end];
            if text != self.target {
                seen.insert(text.to_string());
            } else if !self.accepts(source, start, end) {
                seen.insert(enclosing_word(source, start, end).to_string());
            }
        }
        seen.into_iter().collect()
    }

    fn locate(&self, source: &str) -> Option<Vec<Located>> {
        Some(
            self.occurrences(source)
                .into_iter()
                .map(|(start, end)| Located::at(source, start, end, Vec::new()))
                .collect(),
        )
    }

    fn advice(&self) -> Vec<String> {
        let Some(replacement) = &self.replacement else {
            return Vec::new();
        };
        if self.occurrences(replacement).is_empty() {
            return Vec::new();
        }
        vec![format!(
            "the replacement contains \"{}\" again, so every run rewrites its own output once \
             more; apply it once and remove the rule",
            self.target
        )]
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "text::{} \"{}\"", self.mode.module(), self.target)?;
        match &self.replacement {
            Some(to) => write!(f, " -> replace \"{to}\""),
            None => write!(f, " -> delete"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(mode: Mode, target: &str, to: Option<&str>) -> Literal {
        Literal::new(mode, target.into(), to.map(Into::into)).unwrap()
    }

    #[test]
    fn literal_replaces_every_occurrence() {
        let op = op(Mode::Literal, ".codemod", Some(".colab"));
        assert_eq!(
            op.apply("a.codemod b.codemods // x.codemod\n"),
            "a.colab b.colabs // x.colab\n"
        );
    }

    #[test]
    fn word_leaves_longer_words_alone() {
        let op = op(Mode::Word, ".codemod", Some(".colab"));
        assert_eq!(
            op.apply("\"a.codemod\" b.codemods c.codemod_x d.codemod.\n"),
            "\"a.colab\" b.codemods c.codemod_x d.colab.\n"
        );
    }

    #[test]
    fn word_checks_both_edges_of_a_word_target() {
        let op = op(Mode::Word, "foo", Some("bar"));
        assert_eq!(
            op.apply("foo xfoo foox (foo) foo_ föo foo"),
            "bar xfoo foox (bar) foo_ föo bar"
        );
    }

    #[test]
    fn a_rejected_occurrence_does_not_hide_an_overlapping_one() {
        // `aa` inside `aaa` is never bounded; the trailing ` aa` is.
        let op = op(Mode::Word, "aa", Some("b"));
        assert_eq!(op.apply("aaa aa"), "aaa b");
    }

    #[test]
    fn delete_removes_the_text_only() {
        let op = op(Mode::Literal, " // TODO", None);
        assert_eq!(op.apply("x(); // TODO\ny();\n"), "x();\ny();\n");
    }

    #[test]
    fn unchanged_input_comes_back_as_is() {
        let op = op(Mode::Literal, "absent", Some("x"));
        assert_eq!(op.apply("nothing here"), "nothing here");
    }

    #[test]
    fn empty_and_identity_rules_are_refused() {
        assert!(Literal::new(Mode::Literal, String::new(), None).is_err());
        let err = Literal::new(Mode::Word, "a".into(), Some("a".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("text::word \"a\""), "{err}");
    }

    #[test]
    fn binary_formats_are_not_read() {
        let op = op(Mode::Literal, "x", None);
        assert!(!op.is_file_relevant(Path::new("logo.PNG")));
        assert!(op.is_file_relevant(Path::new("README.md")));
        assert!(op.is_file_relevant(Path::new("Makefile")));
    }

    #[test]
    fn near_misses_are_other_cases_and_longer_words() {
        let op = op(Mode::Word, ".codemod", Some(".colab"));
        assert_eq!(
            op.observed_targets("a.CODEMOD b.codemods ünï.Codemod"),
            vec![".CODEMOD", ".Codemod", "b.codemods"]
        );
    }

    #[test]
    fn a_replacement_that_rematches_is_advised_against() {
        assert!(op(Mode::Literal, "a", Some("ab")).advice().len() == 1);
        assert!(op(Mode::Word, "a", Some("ab")).advice().is_empty());
        assert!(op(Mode::Literal, "a", None).advice().is_empty());
    }

    #[test]
    fn locate_reports_each_occurrence() {
        let op = op(Mode::Word, "x", None);
        let hits = op.locate("x xy\n  x\n").unwrap();
        let at: Vec<(usize, usize)> = hits.iter().map(|h| (h.line, h.column)).collect();
        assert_eq!(at, vec![(1, 1), (2, 3)]);
    }
}
