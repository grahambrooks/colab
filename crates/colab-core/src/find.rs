//! Read-only structural search: where would each rule act?
//!
//! `colab find` / `colab.find` answer "where is this?" before an agent
//! decides what to change. An operation that can say exactly what it
//! matched — a `<lang>::pattern`, with its metavariable bindings —
//! implements [`Operation::locate`]. Every other operation is located
//! generically, by the lines its rewrite would change, so search works for
//! every module without per-backend code.

use std::path::{Path, PathBuf};

use crate::backend::Operation;
use crate::transformer::CodeTransformer;

/// One place a rule matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// Byte range in the source.
    pub start: usize,
    pub end: usize,
    /// 1-based line and column (in characters) of `start`.
    pub line: usize,
    pub column: usize,
    /// The matched source text.
    pub text: String,
    /// Metavariable bindings, when the operation has them.
    pub captures: Vec<(String, String)>,
}

impl Located {
    /// Build a location from a byte range, resolving line and column.
    pub fn at(source: &str, start: usize, end: usize, captures: Vec<(String, String)>) -> Self {
        let before = &source[..start];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        Self {
            start,
            end,
            line,
            column: source[line_start..start].chars().count() + 1,
            text: source[start..end].to_string(),
            captures,
        }
    }
}

/// Where `op` would act in `source`: its own [`Operation::locate`] when it
/// has one, otherwise the original lines its rewrite changes.
pub fn locate(op: &dyn Operation, source: &str) -> Vec<Located> {
    if let Some(found) = op.locate(source) {
        return found;
    }
    let after = op.apply(source);
    if after == source {
        return Vec::new();
    }
    changed_lines(source, &after)
}

/// The original-source line ranges a rewrite touched, one location per
/// contiguous change. A pure insertion is located at the line it follows.
fn changed_lines(before: &str, after: &str) -> Vec<Located> {
    let diff = similar::TextDiff::from_lines(before, after);
    // Byte offset at which each original line starts.
    let mut offsets = vec![0];
    offsets.extend(before.match_indices('\n').map(|(i, _)| i + 1));
    let offset = |line: usize| offsets.get(line).copied().unwrap_or(before.len());

    diff.grouped_ops(0)
        .into_iter()
        .flatten()
        .filter(|op| op.tag() != similar::DiffTag::Equal)
        .map(|op| {
            let range = op.old_range();
            let (start, end) = if range.is_empty() {
                let at = offset(range.start);
                (at, at)
            } else {
                // Exclude the final line's newline from the reported text.
                let end = offset(range.end);
                let end = if end > 0 && before[..end].ends_with('\n') && end > offset(range.start) {
                    end - 1
                } else {
                    end
                };
                (offset(range.start), end)
            };
            Located::at(before, start, end, Vec::new())
        })
        .collect()
}

/// Every location in one file, tagged with the index of the rule that
/// produced it. Rules are searched independently against the original
/// source — `find` reports where each rule *matches*, not the result of
/// running them in sequence.
pub fn locate_all(
    rules: &[Box<dyn Operation>],
    path: &Path,
    source: &str,
) -> Vec<(usize, Located)> {
    let mut out = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        if !rule.is_file_relevant(path) {
            continue;
        }
        if let Some(literal) = rule.prefilter()
            && !source.contains(literal)
        {
            continue;
        }
        out.extend(
            locate(rule.as_ref(), source)
                .into_iter()
                .map(|l| (index, l)),
        );
    }
    out
}

/// A [`CodeTransformer`] that selects the files any rule is relevant to
/// and changes nothing — so the ordinary walker (globs, `.gitignore`,
/// parallel reads) can drive a read-only search.
pub struct Selector<'a> {
    pub rules: &'a [Box<dyn Operation>],
}

impl CodeTransformer for Selector<'_> {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.rules.iter().any(|r| r.is_file_relevant(path))
    }

    fn apply(&self, source_code: &str) -> String {
        source_code.to_string()
    }
}

/// One search hit, for rendering.
#[derive(Debug, Clone)]
pub struct Hit {
    pub path: PathBuf,
    pub rule: usize,
    pub located: Located,
}

/// How much of each hit to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FindDetail {
    /// `path:line:col: first line of the match` — the cheap default.
    #[default]
    Concise,
    /// Adds metavariable captures and the full (capped) match text.
    Detailed,
}

impl FindDetail {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "concise" => Some(Self::Concise),
            "detailed" => Some(Self::Detailed),
            _ => None,
        }
    }
}

/// Default cap on hits returned; the remainder is counted, not listed.
pub const DEFAULT_MAX_RESULTS: usize = 50;

/// Accumulates a search: every hit is counted, the first `max_results`
/// are kept. Per-rule totals and the walk counters live in the embedded
/// [`RunReport`] so dead-rule warnings and near misses work exactly as
/// they do for a rewrite — `files_matched` there means "files with a
/// hit", and `files_changed` "files with any hit".
///
/// [`RunReport`]: crate::report::RunReport
#[derive(Debug)]
pub struct FindReport {
    pub run: crate::report::RunReport,
    pub hits: Vec<Hit>,
    /// Hits per rule, by rule index.
    pub matches_per_rule: Vec<u64>,
    pub total: u64,
    pub max_results: usize,
    pub detail: FindDetail,
}

impl FindReport {
    pub fn new(rules: &[Box<dyn Operation>], max_results: usize, detail: FindDetail) -> Self {
        Self {
            run: crate::report::RunReport::with_rules(rules.iter().map(|r| r.to_string())),
            hits: Vec::new(),
            matches_per_rule: vec![0; rules.len()],
            total: 0,
            max_results,
            detail,
        }
    }

    /// Search one file and fold its hits in.
    pub fn record(&mut self, rules: &[Box<dyn Operation>], path: &Path, source: &str) {
        self.run.record(&crate::walker::FileChange {
            path: path.to_path_buf(),
            before: String::new(),
            after: String::new(),
            rules_fired: Vec::new(),
        });
        let found = locate_all(rules, path, source);
        if found.is_empty() {
            return;
        }
        self.run.files_changed += 1;
        let mut fired = vec![false; rules.len()];
        for (rule, located) in found {
            self.total += 1;
            self.matches_per_rule[rule] += 1;
            fired[rule] = true;
            if self.hits.len() < self.max_results {
                self.hits.push(Hit {
                    path: path.to_path_buf(),
                    rule,
                    located,
                });
            }
        }
        for (index, hit) in fired.into_iter().enumerate() {
            if hit {
                self.run.rules[index].files_matched += 1;
            }
        }
    }

    /// Hits counted but not listed.
    pub fn truncated(&self) -> u64 {
        self.total - self.hits.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    /// Renames `old` to `new` textually; has no `locate` of its own.
    #[derive(Debug)]
    struct Rename;

    impl fmt::Display for Rename {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "rename")
        }
    }

    impl Operation for Rename {
        fn is_file_relevant(&self, path: &Path) -> bool {
            path.extension().is_some_and(|e| e == "txt")
        }
        fn apply(&self, source: &str) -> String {
            source.replace("old", "new")
        }
        fn prefilter(&self) -> Option<&str> {
            Some("old")
        }
    }

    #[test]
    fn fallback_locates_the_changed_lines() {
        let src = "a\nuse old;\nb\nc\nold();\n";
        let found = locate(&Rename, src);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].line, found[0].column), (2, 1));
        assert_eq!(found[0].text, "use old;");
        assert_eq!(found[1].line, 5);
        assert_eq!(found[1].text, "old();");
    }

    #[test]
    fn nothing_to_change_locates_nothing() {
        assert!(locate(&Rename, "nothing here\n").is_empty());
    }

    #[test]
    fn locate_all_respects_relevance_and_prefilter() {
        let rules: Vec<Box<dyn Operation>> = vec![Box::new(Rename)];
        assert!(locate_all(&rules, Path::new("a.rs"), "old").is_empty());
        assert!(locate_all(&rules, Path::new("a.txt"), "nothing").is_empty());
        assert_eq!(locate_all(&rules, Path::new("a.txt"), "old\n").len(), 1);
    }

    #[test]
    fn located_at_counts_columns_in_characters() {
        let l = Located::at("µµ x\n", "µµ ".len(), "µµ x".len(), Vec::new());
        assert_eq!((l.line, l.column, l.text.as_str()), (1, 4, "x"));
    }
}
