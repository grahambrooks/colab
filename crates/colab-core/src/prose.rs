//! `and prose`: a `symbol` rename that also renames the old name where it
//! appears, as a whole word, in comments and string contents.
//!
//! The backend says where its prose is (`LanguageBackend::prose_ranges`,
//! from its own grammar); this wrapper runs the rename, then rewrites the
//! remaining whole-word matches inside those ranges. Code that is not
//! prose is left to the rename, so a different symbol with the same name
//! in code is never touched by this step.

use std::fmt;
use std::ops::Range;
use std::path::Path;

use crate::backend::Operation;

/// Where a source file's comments and string contents are.
pub type ProseLocator = fn(&str) -> Option<Vec<Range<usize>>>;

/// A rename followed by the same rename in comments and strings.
pub struct ProseRename {
    inner: Box<dyn Operation>,
    from: String,
    to: String,
    locate: ProseLocator,
}

impl ProseRename {
    pub fn new(inner: Box<dyn Operation>, from: &str, to: &str, locate: ProseLocator) -> Self {
        Self {
            inner,
            from: from.to_string(),
            to: to.to_string(),
            locate,
        }
    }
}

/// `text` with every whole-word `from` inside `ranges` made `to`.
pub fn replace_words(text: &str, from: &str, to: &str, ranges: &[Range<usize>]) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, _) in text.match_indices(from) {
        let end = at + from.len();
        let bounded = !text[..at].chars().next_back().is_some_and(is_word)
            && !text[end..].chars().next().is_some_and(is_word);
        if bounded && at >= last && ranges.iter().any(|r| r.start <= at && end <= r.end) {
            out.push_str(&text[last..at]);
            out.push_str(to);
            last = end;
        }
    }
    out.push_str(&text[last..]);
    out
}

impl fmt::Debug for ProseRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProseRename({:?})", self.inner)
    }
}

impl fmt::Display for ProseRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} and prose", self.inner)
    }
}

impl Operation for ProseRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.inner.is_file_relevant(path)
    }

    fn apply(&self, source: &str) -> String {
        let renamed = self.inner.apply(source);
        match (self.locate)(&renamed) {
            Some(ranges) => replace_words(&renamed, &self.from, &self.to, &ranges),
            None => renamed,
        }
    }

    /// The old name must appear, in code or in prose.
    fn prefilter(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn near_miss_target(&self) -> Option<&str> {
        self.inner.near_miss_target()
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        self.inner.observed_targets(source)
    }

    fn conflicts(&self, before: &str, after: &str) -> Vec<String> {
        self.inner.conflicts(before, after)
    }

    fn creates(&self, path: &std::path::Path, source: &str) -> Vec<crate::Creation> {
        self.inner.creates(path, source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whole_words_inside_the_ranges_change() {
        let text = "// Parse it\nParse(); // Parser and Parse\n\"Parse\"";
        let ranges = [0..11, 21..40, 41..48];
        assert_eq!(
            replace_words(text, "Parse", "Load", &ranges),
            "// Load it\nParse(); // Parser and Load\n\"Load\""
        );
    }
}
