//! Running a source rule inside Markdown code fences.
//!
//! A rule written `match go::pattern '…' in fences { … }` (or `and
//! fences`, which also keeps the rule for Go files) is wrapped in a
//! [`FencedOperation`]. In a Markdown file it finds the fences whose info
//! string names the rule's language (```` ```go ````, ```` ```golang ````),
//! runs the rule on each fence's text, and puts the result back. A fence
//! indented inside a list item is dedented first and re-indented after, so
//! the rule sees the code as it would in a source file.
//!
//! Finding fences needs the Markdown grammar, which `colab-core` does not
//! link; the Markdown backend supplies it as a [`FenceLocator`].

use std::fmt;
use std::ops::Range;
use std::path::Path;

use crate::backend::Operation;
use crate::find::Located;

/// One fenced code block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence {
    /// The first word of the info string, lowercased: `go` for
    /// ```` ```go title="x" ````. Empty for a bare fence.
    pub lang: String,
    /// The code between the fence lines, indentation included.
    pub content: Range<usize>,
    /// The opening fence line's indentation, stripped from each code line.
    pub indent: String,
}

/// Finds the fenced code blocks in a Markdown file.
pub type FenceLocator = fn(&str) -> Vec<Fence>;

/// A rule applied to the code fences of Markdown files.
pub struct FencedOperation {
    inner: Box<dyn Operation>,
    names: Vec<&'static str>,
    locate: FenceLocator,
}

impl FencedOperation {
    /// Run `inner` on the fences whose language is one of `names`.
    pub fn new(inner: Box<dyn Operation>, names: Vec<&'static str>, locate: FenceLocator) -> Self {
        Self {
            inner,
            names,
            locate,
        }
    }

    /// Each matching fence: its range in `source` and its dedented code.
    fn fences(&self, source: &str) -> Vec<(Fence, String)> {
        (self.locate)(source)
            .into_iter()
            .filter(|f| self.names.iter().any(|n| n.eq_ignore_ascii_case(&f.lang)))
            .map(|f| {
                let code = dedent(&source[f.content.clone()], &f.indent);
                (f, code)
            })
            .collect()
    }
}

/// Strip `indent` from the start of every line that has it.
fn dedent(code: &str, indent: &str) -> String {
    if indent.is_empty() {
        return code.to_string();
    }
    code.split_inclusive('\n')
        .map(|line| line.strip_prefix(indent).unwrap_or(line))
        .collect()
}

/// Put `indent` back on every non-empty line.
fn reindent(code: &str, indent: &str) -> String {
    if indent.is_empty() {
        return code.to_string();
    }
    code.split_inclusive('\n')
        .map(|line| {
            if line.trim().is_empty() {
                line.to_string()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect()
}

/// The byte in `original` (an indented fence body starting at `base`)
/// that byte `at` of its dedented text came from.
fn map_offset(original: &str, base: usize, indent: &str, at: usize) -> usize {
    let mut seen = 0;
    let mut out = base;
    for line in original.split_inclusive('\n') {
        let stripped = line.strip_prefix(indent).unwrap_or(line);
        let shift = line.len() - stripped.len();
        if at < seen + stripped.len() {
            return out + shift + (at - seen);
        }
        seen += stripped.len();
        out += line.len();
    }
    out
}

impl fmt::Debug for FencedOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FencedOperation({:?})", self.inner)
    }
}

impl fmt::Display for FencedOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} in fences", self.inner)
    }
}

impl Operation for FencedOperation {
    fn is_file_relevant(&self, path: &Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("md" | "markdown")
        )
    }

    fn apply(&self, source: &str) -> String {
        let mut out = source.to_string();
        // Last fence first, so earlier ranges stay valid.
        for (fence, code) in self.fences(source).into_iter().rev() {
            let changed = self.inner.apply(&code);
            if changed != code {
                out.replace_range(fence.content.clone(), &reindent(&changed, &fence.indent));
            }
        }
        out
    }

    /// The rule's own literal: a fence is part of the file, so a literal
    /// absent from the file is absent from every fence.
    fn prefilter(&self) -> Option<&str> {
        self.inner.prefilter()
    }

    fn near_miss_target(&self) -> Option<&str> {
        self.inner.near_miss_target()
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        self.fences(source)
            .iter()
            .flat_map(|(_, code)| self.inner.observed_targets(code))
            .collect()
    }

    fn locate(&self, source: &str) -> Option<Vec<Located>> {
        let mut out = Vec::new();
        for (fence, code) in self.fences(source) {
            let original = &source[fence.content.clone()];
            for hit in crate::find::locate(self.inner.as_ref(), &code) {
                let start = map_offset(original, fence.content.start, &fence.indent, hit.start);
                let end = map_offset(original, fence.content.start, &fence.indent, hit.end);
                out.push(Located::at(source, start, end, hit.captures));
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Upper-cases `foo` wherever it appears.
    #[derive(Debug)]
    struct Shout;

    impl fmt::Display for Shout {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "go::shout")
        }
    }

    impl Operation for Shout {
        fn is_file_relevant(&self, path: &Path) -> bool {
            path.extension().is_some_and(|e| e == "go")
        }
        fn apply(&self, source: &str) -> String {
            source.replace("foo", "FOO")
        }
        fn prefilter(&self) -> Option<&str> {
            Some("foo")
        }
    }

    /// A toy locator: ```` ```lang ```` fences, possibly indented.
    fn toy(source: &str) -> Vec<Fence> {
        let mut out = Vec::new();
        let mut at = 0;
        let mut open: Option<(String, String, usize)> = None;
        for line in source.split_inclusive('\n') {
            let trimmed = line.trim_start();
            if let Some(info) = trimmed.strip_prefix("```") {
                match open.take() {
                    None => {
                        let indent = line[..line.len() - trimmed.len()].to_string();
                        open = Some((info.trim().to_lowercase(), indent, at + line.len()));
                    }
                    Some((lang, indent, start)) => out.push(Fence {
                        lang,
                        content: start..at,
                        indent,
                    }),
                }
            }
            at += line.len();
        }
        out
    }

    fn op() -> FencedOperation {
        FencedOperation::new(Box::new(Shout), vec!["go", "golang"], toy)
    }

    #[test]
    fn rewrites_only_fences_in_the_rules_language() {
        let doc = "foo\n```go\nfoo()\n```\n```sh\nfoo\n```\n```Golang\nx := foo\n```\n";
        assert_eq!(
            op().apply(doc),
            "foo\n```go\nFOO()\n```\n```sh\nfoo\n```\n```Golang\nx := FOO\n```\n"
        );
        assert!(op().is_file_relevant(Path::new("README.md")));
        assert!(!op().is_file_relevant(Path::new("main.go")));
        assert_eq!(op().to_string(), "go::shout in fences");
    }

    #[test]
    fn an_indented_fence_is_dedented_for_the_rule_and_restored() {
        let doc = "- step\n\n  ```go\n  foo()\n\n  bar(foo)\n  ```\n";
        let out = op().apply(doc);
        assert_eq!(out, "- step\n\n  ```go\n  FOO()\n\n  bar(FOO)\n  ```\n");
        let hits = op().locate(doc).unwrap();
        // `Shout` has no `locate`, so hits are its changed lines, mapped
        // past the indentation.
        assert_eq!(
            hits.iter().map(|h| (h.line, h.column)).collect::<Vec<_>>(),
            [(4, 3), (6, 3)]
        );
    }
}
