//! Links in a Markdown file, and `markdown::link "<destination>"`:
//! `replace` or `delete` link destinations.
//!
//! A destination is matched three ways:
//!
//! | Target | Matches | `replace` rewrites |
//! | ------ | ------- | ------------------ |
//! | `#old` | the same-file anchor `#old` | the whole destination |
//! | `guide.md` | `guide.md`, `../docs/guide.md`, `guide.md#x`: the path's trailing segments | that path suffix, keeping any prefix and anchor |
//! | `guide.md#old` | the path as above, with exactly that anchor | the suffix and the anchor |
//!
//! Links are matched as written, relative to their own file, so a target
//! names the trailing segments every file shares; narrow with `in` when
//! two files of the same name exist. Code spans and fences are not links.
//! `delete` unwraps an inline link to its text, removes an image, and
//! removes a reference definition's line.

use std::fmt;
use std::ops::Range;
use std::path::Path;

use colab_core::{Error, Operation, Result};

/// What kind of link a destination belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `[text](dest)`
    Inline,
    /// `![alt](dest)`
    Image,
    /// `[label]: dest`
    Definition,
}

/// One link destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub kind: Kind,
    pub dest: String,
    /// The destination's bytes in the file.
    pub range: Range<usize>,
    /// The whole link (or definition line), and for an inline link its
    /// text, for `delete`.
    pub whole: Range<usize>,
    pub text: Option<Range<usize>>,
    /// 1-based.
    pub line: usize,
}

/// Every link destination in `source`, in order.
pub fn links(source: &str) -> Vec<Link> {
    let Some(tree) = crate::parse(source) else {
        return Vec::new();
    };
    let line_of = |at: usize| source[..at].matches('\n').count() + 1;
    let mut out = Vec::new();
    for node in crate::nodes(&tree, |n| {
        matches!(
            n.kind_name(),
            "inline" | "pipe_table_cell" | "link_reference_definition"
        )
    }) {
        let base = node.start();
        if node.kind_name() == "link_reference_definition" {
            if let Some(dest) = node
                .children()
                .find(|n| n.kind_name() == "link_destination")
            {
                let line_start = source[..base].rfind('\n').map_or(0, |i| i + 1);
                let line_end = source[node.end()..]
                    .find('\n')
                    .map_or(source.len(), |i| node.end() + i + 1);
                let range = dest.range();
                out.push(Link {
                    kind: Kind::Definition,
                    dest: source[range.clone()].to_string(),
                    line: line_of(range.start),
                    range,
                    whole: line_start..line_end.min(source.len()),
                    text: None,
                });
            }
            continue;
        }
        let text = &source[node.range()];
        // Every link and image starts with `[` or `![`; most paragraphs
        // have none, and an inline parse costs more than the scan.
        if !text.contains('[') {
            continue;
        }
        let Some(inline) = crate::parse_inline(text) else {
            continue;
        };
        for link in crate::nodes(&inline, |n| {
            matches!(n.kind_name(), "inline_link" | "image")
        }) {
            let children: Vec<_> = link.children().collect();
            let Some(dest) = children
                .iter()
                .find(|n| n.kind_name() == "link_destination")
            else {
                continue;
            };
            let shift = |r: Range<usize>| base + r.start..base + r.end;
            let range = shift(dest.range());
            out.push(Link {
                kind: if link.kind_name() == "image" {
                    Kind::Image
                } else {
                    Kind::Inline
                },
                dest: source[range.clone()].to_string(),
                line: line_of(range.start),
                range,
                whole: shift(link.range()),
                text: children
                    .iter()
                    .find(|n| n.kind_name() == "link_text")
                    .map(|n| shift(n.range())),
            });
        }
    }
    out.sort_by_key(|l| l.range.start);
    out
}

/// Where in `dest` the target `target` matches: the byte range to
/// replace, or `None`.
fn match_dest(target: &str, dest: &str) -> Option<Range<usize>> {
    if target.starts_with('#') {
        return (dest == target).then_some(0..dest.len());
    }
    let (dest_path, dest_anchor) = match dest.find('#') {
        Some(i) => (&dest[..i], Some(&dest[i..])),
        None => (dest, None),
    };
    let (target_path, target_anchor) = match target.find('#') {
        Some(i) => (&target[..i], Some(&target[i..])),
        None => (target, None),
    };
    let start = path_suffix(dest_path, target_path.trim_start_matches("./"))?;
    match target_anchor {
        None => Some(start..dest_path.len()),
        Some(anchor) => (dest_anchor == Some(anchor)).then_some(start..dest.len()),
    }
}

/// The offset in `path` where `suffix` starts as whole trailing segments.
fn path_suffix(path: &str, suffix: &str) -> Option<usize> {
    if suffix.is_empty() {
        return None;
    }
    let start = path.len().checked_sub(suffix.len())?;
    (path.ends_with(suffix) && (start == 0 || path[..start].ends_with('/'))).then_some(start)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Replace(String),
    Delete,
}

/// One `markdown::link` rule.
#[derive(Debug)]
pub struct LinkEdit {
    target: String,
    mode: Mode,
}

impl LinkEdit {
    pub fn replace(target: &str, with: &str) -> Result<Self> {
        Self::build(target, Mode::Replace(with.trim().to_string()))
    }

    pub fn delete(target: &str) -> Result<Self> {
        Self::build(target, Mode::Delete)
    }

    fn build(target: &str, mode: Mode) -> Result<Self> {
        let target = target.trim();
        if target.is_empty() || target == "#" || target.contains(char::is_whitespace) {
            return Err(Error::Config(format!(
                "markdown::link \"{target}\": give a destination such as `guide.md`, `guide.md#usage` or `#usage`"
            )));
        }
        Ok(Self {
            target: target.to_string(),
            mode,
        })
    }

    fn edit(&self, source: &str) -> Option<String> {
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        for link in links(source) {
            let Some(hit) = match_dest(&self.target, &link.dest) else {
                continue;
            };
            match &self.mode {
                Mode::Replace(with) => {
                    let range = link.range.start + hit.start..link.range.start + hit.end;
                    if source[range.clone()] != *with {
                        edits.push((range, with.clone()));
                    }
                }
                Mode::Delete => edits.push(match (link.kind, &link.text) {
                    (Kind::Inline, Some(text)) => {
                        (link.whole.clone(), source[text.clone()].to_string())
                    }
                    _ => (link.whole.clone(), String::new()),
                }),
            }
        }
        if edits.is_empty() {
            return None;
        }
        let mut out = source.to_string();
        for (range, with) in edits.into_iter().rev() {
            out.replace_range(range, &with);
        }
        Some(out)
    }
}

impl fmt::Display for LinkEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.mode {
            Mode::Replace(to) => write!(f, "markdown::link \"{}\" -> \"{to}\"", self.target),
            Mode::Delete => write!(f, "markdown::link \"{}\" -> delete", self.target),
        }
    }
}

impl Operation for LinkEdit {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_markdown(path)
    }

    fn apply(&self, source: &str) -> String {
        self.edit(source).unwrap_or_else(|| source.to_string())
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.target)
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.target)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        links(source).into_iter().map(|l| l.dest).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# A\n\nSee [the guide](../docs/guide.md#usage), [top](#a) and ![logo](img/guide.md.png).\n\n| Doc | Link |\n| - | - |\n| g | [g](guide.md) |\n\n`[not](guide.md)`\n\n[ref]: docs/guide.md#install\n";

    #[test]
    fn links_are_found_everywhere_but_code() {
        let found: Vec<(Kind, String, usize)> = links(DOC)
            .into_iter()
            .map(|l| (l.kind, l.dest, l.line))
            .collect();
        assert_eq!(
            found,
            [
                (Kind::Inline, "../docs/guide.md#usage".into(), 3),
                (Kind::Inline, "#a".into(), 3),
                (Kind::Image, "img/guide.md.png".into(), 3),
                (Kind::Inline, "guide.md".into(), 7),
                (Kind::Definition, "docs/guide.md#install".into(), 11),
            ]
        );
    }

    #[test]
    fn replace_rewrites_the_matched_suffix_and_keeps_anchors() {
        let out = LinkEdit::replace("guide.md", "manual.md")
            .unwrap()
            .apply(DOC);
        assert!(out.contains("(../docs/manual.md#usage)"), "{out}");
        assert!(out.contains("[g](manual.md)"));
        assert!(out.contains("[ref]: docs/manual.md#install"));
        assert!(out.contains("(img/guide.md.png)"), "not a whole segment");
        assert!(out.contains("`[not](guide.md)`"), "code is not a link");

        let anchor = LinkEdit::replace("guide.md#usage", "guide.md#use")
            .unwrap()
            .apply(DOC);
        assert!(
            anchor.contains("(../docs/guide.md#use)") && anchor.contains("docs/guide.md#install")
        );
        let local = LinkEdit::replace("#a", "#b").unwrap();
        let out = local.apply(DOC);
        assert!(out.contains("[top](#b)"));
        assert_eq!(local.apply(&out), out);
    }

    #[test]
    fn delete_unwraps_links_and_drops_definitions() {
        let out = LinkEdit::delete("guide.md").unwrap().apply(DOC);
        assert!(out.contains("See the guide, [top](#a)"), "{out}");
        assert!(out.contains("| g | g |"));
        assert!(!out.contains("[ref]:"));
    }
}
