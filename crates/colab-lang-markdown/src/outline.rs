//! A Markdown file's headings and the sections they open, and resolving a
//! section address (`"Parent > Child"` or `"#slug"`) against them.
//!
//! A section runs from its heading to the next heading of the same or a
//! higher level, so it includes its subsections, as a reader sees it.
//! Sections are computed from the headings rather than taken from the
//! grammar's `section` node, which does not open one for a setext
//! heading. Headings inside code fences are code, and the grammar never
//! reports them.

use std::ops::Range;

/// One heading and the section it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 1 to 6.
    pub level: u8,
    /// The heading as a reader sees it: inline markup (`*`, `` ` ``, link
    /// syntax) and closing `#`s removed.
    pub text: String,
    /// The GitHub-style anchor, made unique within the file (`-1`, `-2`).
    pub slug: String,
    /// The texts of the enclosing headings, outermost first, then this one.
    pub path: Vec<String>,
    /// The heading text as written, for renaming it in place.
    pub content: Range<usize>,
    /// Byte where the heading's first line starts.
    pub start: usize,
    /// Byte just past the heading (its line, or its setext underline).
    pub heading_end: usize,
    /// Byte where the section ends: the next heading of this level or
    /// higher, or the end of the file.
    pub end: usize,
    /// 1-based lines of `start` and of the section's last line.
    pub line: usize,
    pub end_line: usize,
    /// Tables, lists and code fences in the section, subsections included.
    pub tables: usize,
    pub lists: usize,
    pub fences: usize,
}

impl Heading {
    /// `Parent > Child`, the address that names this section.
    pub fn address(&self) -> String {
        self.path.join(" > ")
    }
}

/// Every heading in `source`, in order. Empty if it does not parse.
pub fn headings(source: &str) -> Vec<Heading> {
    let Some(tree) = crate::parse(source) else {
        return Vec::new();
    };
    let mut found: Vec<Heading> = Vec::new();
    let mut blocks: Vec<(&'static str, usize)> = Vec::new();
    colab_rewrite::visit_all(&tree, |node| {
        let kind = node.kind();
        match kind {
            "atx_heading" | "setext_heading" => {
                let level = heading_level(node, source);
                let Some(content) = node.child_by_field_name("heading_content") else {
                    return;
                };
                let range = trim_range(source, content.byte_range());
                let range = strip_closing_hashes(source, range);
                found.push(Heading {
                    level,
                    text: plain_text(&source[range.clone()]),
                    slug: String::new(),
                    path: Vec::new(),
                    content: range,
                    start: line_start(source, node.start_byte()),
                    heading_end: node.end_byte(),
                    end: source.len(),
                    line: node.start_position().row + 1,
                    end_line: 0,
                    tables: 0,
                    lists: 0,
                    fences: 0,
                });
            }
            "pipe_table" => blocks.push(("table", node.start_byte())),
            "fenced_code_block" => blocks.push(("fence", node.start_byte())),
            // Only outermost lists: a nested list is part of its item.
            "list"
                if !std::iter::successors(node.parent(), |n| n.parent())
                    .any(|n| n.kind() == "list") =>
            {
                blocks.push(("list", node.start_byte()))
            }
            _ => {}
        }
    });

    // Section ends, paths and slugs.
    let mut stack: Vec<usize> = Vec::new();
    let mut slugs: std::collections::HashMap<String, usize> = Default::default();
    for i in 0..found.len() {
        let level = found[i].level;
        found[i].end = found[i + 1..]
            .iter()
            .find(|h| h.level <= level)
            .map_or(source.len(), |h| h.start);
        while stack.last().is_some_and(|&j| found[j].level >= level) {
            stack.pop();
        }
        let mut path: Vec<String> = stack.iter().map(|&j| found[j].text.clone()).collect();
        path.push(found[i].text.clone());
        found[i].path = path;
        stack.push(i);

        let base = slug(&found[i].text);
        let n = slugs.entry(base.clone()).or_insert(0);
        found[i].slug = if *n == 0 { base } else { format!("{base}-{n}") };
        *n += 1;

        let last = source[..found[i].end]
            .trim_end_matches(['\n', '\r', ' ', '\t'])
            .len();
        found[i].end_line = source[..last.max(found[i].start)].matches('\n').count() + 1;
        let span = found[i].start..found[i].end;
        for (kind, at) in &blocks {
            if span.contains(at) {
                match *kind {
                    "table" => found[i].tables += 1,
                    "fence" => found[i].fences += 1,
                    _ => found[i].lists += 1,
                }
            }
        }
    }
    found
}

fn heading_level(node: tree_sitter::Node<'_>, source: &str) -> u8 {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if let Some(n) = kind
            .strip_prefix("atx_h")
            .and_then(|k| k.strip_suffix("_marker"))
        {
            return n.parse().unwrap_or(1);
        }
        if kind == "setext_h1_underline" {
            return 1;
        }
        if kind == "setext_h2_underline" {
            return 2;
        }
    }
    let _ = source;
    1
}

/// Where `address` points in `source`: exactly one heading, or an error
/// naming the candidates.
pub fn resolve<'h>(headings: &'h [Heading], address: &str) -> Result<&'h Heading, String> {
    let matches = find(headings, address);
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "no section `{address}`{}",
            nearest(headings, address)
                .map(|n| format!("; did you mean `{n}`?"))
                .unwrap_or_default()
        )),
        many => Err(format!(
            "`{address}` is ambiguous: it matches {}; give more of its path",
            many.iter()
                .map(|h| format!("`{}` (line {})", h.address(), h.line))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Every heading `address` names. `#slug` matches an anchor; otherwise
/// the segments of `A > B > C` must be the heading's text and, in order,
/// texts of headings enclosing it (not necessarily its direct parents).
pub fn find<'h>(headings: &'h [Heading], address: &str) -> Vec<&'h Heading> {
    if let Some(slug) = address.strip_prefix('#') {
        return headings.iter().filter(|h| h.slug == slug).collect();
    }
    let segments = segments(address);
    let Some((last, outer)) = segments.split_last() else {
        return Vec::new();
    };
    headings
        .iter()
        .filter(|h| h.text == *last && encloses(&h.path[..h.path.len() - 1], outer))
        .collect()
}

/// The segments of `A > B`, trimmed.
pub fn segments(address: &str) -> Vec<String> {
    address
        .split(" > ")
        .map(|s| plain_text(s.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// `true` when `outer` appears in `ancestors` in order.
fn encloses(ancestors: &[String], outer: &[String]) -> bool {
    let mut rest = ancestors.iter();
    outer.iter().all(|want| rest.any(|a| a == want))
}

fn nearest(headings: &[Heading], address: &str) -> Option<String> {
    let last = segments(address).pop()?;
    let texts: Vec<&str> = headings.iter().map(|h| h.text.as_str()).collect();
    let near = colab_core::suggest::closest(&last, texts)?;
    headings
        .iter()
        .find(|h| h.text == near)
        .map(Heading::address)
}

/// A heading as a reader sees it: no emphasis or code markers, a link as
/// its text, collapsed whitespace.
pub fn plain_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '`' => {}
            '!' if chars.peek() == Some(&'[') => {}
            '[' => {}
            ']' if chars.peek() == Some(&'(') => {
                // Skip the destination.
                let mut depth = 0;
                for d in chars.by_ref() {
                    match d {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            ']' => {}
            _ => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// GitHub's anchor for a heading's text: lowercase, punctuation other than
/// `-` and `_` dropped, each space a `-`.
pub fn slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

fn line_start(source: &str, at: usize) -> usize {
    source[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn trim_range(source: &str, range: Range<usize>) -> Range<usize> {
    let text = &source[range.clone()];
    let start = range.start + (text.len() - text.trim_start().len());
    let end = range.start + text.trim_end().len();
    start..end.max(start)
}

/// `## Title ##` → `Title`.
fn strip_closing_hashes(source: &str, range: Range<usize>) -> Range<usize> {
    let text = &source[range.clone()];
    let trimmed = text.trim_end_matches('#');
    if trimmed.len() < text.len() && (trimmed.is_empty() || trimmed.ends_with([' ', '\t'])) {
        let end = range.start + trimmed.trim_end().len();
        return range.start..end;
    }
    range
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "---\ntitle: x\n---\n# Title\n\nIntro.\n\n## A *b* `c`\n\nText.\n\n### Deep\n\n- one\n\n```sh\n# not a heading\n```\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nSetext\n------\n\nBody.\n\n## Deep\n";

    #[test]
    fn headings_carry_levels_paths_slugs_and_spans() {
        let hs = headings(DOC);
        let addresses: Vec<String> = hs.iter().map(Heading::address).collect();
        assert_eq!(
            addresses,
            [
                "Title",
                "Title > A b c",
                "Title > A b c > Deep",
                "Title > Setext",
                "Title > Deep"
            ]
        );
        assert_eq!(hs[1].slug, "a-b-c");
        assert_eq!(hs[4].slug, "deep-1", "duplicate anchors are numbered");
        assert_eq!(&DOC[hs[1].content.clone()], "A *b* `c`");
        // `### Deep` holds the list, the fence and the table; the `#` in
        // the fence is not a heading.
        assert_eq!((hs[2].lists, hs[2].fences, hs[2].tables), (1, 1, 1));
        assert!(DOC[hs[3].start..hs[3].end].starts_with("Setext\n------\n"));
        assert_eq!(hs[0].end, DOC.len());
        assert_eq!(hs[0].line, 4);
    }

    #[test]
    fn addresses_resolve_by_path_or_slug_and_name_the_candidates() {
        let hs = headings(DOC);
        assert_eq!(resolve(&hs, "A b c > Deep").unwrap().line, 12);
        assert!(
            resolve(&hs, "Title > Deep")
                .unwrap_err()
                .contains("ambiguous")
        );
        assert_eq!(resolve(&hs, "#deep-1").unwrap().line, 29);
        assert_eq!(resolve(&hs, "Setext").unwrap().level, 2);
        let err = resolve(&hs, "Setxt").unwrap_err();
        assert!(err.contains("did you mean `Title > Setext`"), "{err}");
        let err = resolve(&hs, "Deep").unwrap_err();
        assert!(
            err.contains("(line 12)") && err.contains("(line 29)"),
            "{err}"
        );
    }

    #[test]
    fn plain_text_and_slugs_follow_what_a_reader_sees() {
        assert_eq!(
            plain_text("Use `colab rename` [here](a.md#x)"),
            "Use colab rename here"
        );
        assert_eq!(slug("M28 — Markdown structure"), "m28--markdown-structure");
        assert_eq!(slug("Token cost (M27 + M22)"), "token-cost-m27--m22");
        assert_eq!(slug("prose_ranges"), "prose_ranges");
    }
}
