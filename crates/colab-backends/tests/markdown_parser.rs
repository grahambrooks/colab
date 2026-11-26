//! `colab_parse_markdown`, colab's own Markdown parser, held to what the
//! Markdown backend read from tree-sitter-md, over every `.md` file in the
//! repo (or under `COLAB_MD_FILES`, a directory, for a wider run). The
//! trees are shaped differently on purpose (colab's has no `section`
//! nodes, continuation tokens or inline punctuation), so what is compared
//! is what the backend takes from them: each heading's level and text,
//! each outermost list's items (marker, text, span), each pipe table's
//! cells, each fence's delimiters and language, and every link destination
//! (definitions, inline links and images, by the inline parse of each
//! paragraph, heading and cell). Each tree must also be lossless.

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use colab_syntax::{Node, Tree};

/// One thing the backend reads, as comparable data.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Fact {
    Heading {
        level: u8,
        content: Span,
    },
    Item {
        list: usize,
        marker: Span,
        text: Span,
        end: usize,
    },
    Cell {
        table: usize,
        cell: Span,
    },
    Fence {
        delimiters: Vec<Span>,
        language: String,
    },
    Link {
        kind: &'static str,
        dest: Span,
    },
}

/// A byte range, ordered.
type Span = (usize, usize);

fn span(r: Range<usize>) -> Span {
    (r.start, r.end)
}

fn trim(source: &str, r: Range<usize>) -> Span {
    let s = &source[r.clone()];
    let start = r.start + (s.len() - s.trim_start().len());
    (start, (start + s.trim().len()).max(start))
}

fn children<'t>(n: Node<'t>, kind: &str) -> Vec<Node<'t>> {
    n.children().filter(|c| c.kind_name() == kind).collect()
}

/// What the backend reads from a block tree, links found by `inline`.
fn facts(source: &str, tree: &Tree, inline: &dyn Fn(&str) -> Option<Tree>) -> BTreeSet<Fact> {
    let mut out = BTreeSet::new();
    for n in tree.nodes() {
        match n.kind_name() {
            "atx_heading" | "setext_heading" => {
                let level = n
                    .children()
                    .find_map(|c| {
                        let k = c.kind_name();
                        if let Some(l) = k
                            .strip_prefix("atx_h")
                            .and_then(|k| k.strip_suffix("_marker"))
                        {
                            return l.parse().ok();
                        }
                        match k {
                            "setext_h1_underline" => Some(1),
                            "setext_h2_underline" => Some(2),
                            _ => None,
                        }
                    })
                    .unwrap_or(1);
                if let Some(content) = n.child_by_field("heading_content") {
                    out.insert(Fact::Heading {
                        level,
                        content: trim(source, content.range()),
                    });
                }
            }
            "list"
                if !std::iter::successors(n.parent(), Node::parent)
                    .any(|p| p.kind_name() == "list") =>
            {
                for item in children(n, "list_item") {
                    let Some(marker) = item
                        .children()
                        .find(|c| c.kind_name().starts_with("list_marker"))
                    else {
                        continue;
                    };
                    let text = item
                        .children()
                        .find(|c| c.kind_name() == "paragraph")
                        .and_then(|p| p.children().find(|c| c.kind_name() == "inline"))
                        .map_or(marker.end()..marker.end(), |i| i.range());
                    let end = source[..item.end()].trim_end().len();
                    out.insert(Fact::Item {
                        list: n.start(),
                        marker: trim(source, marker.range()),
                        text: trim(source, text),
                        end,
                    });
                }
            }
            "pipe_table" => {
                for row in n.children() {
                    for cell in children(row, "pipe_table_cell") {
                        out.insert(Fact::Cell {
                            table: n.start(),
                            cell: trim(source, cell.range()),
                        });
                    }
                }
            }
            "fenced_code_block" => {
                let delimiters = children(n, "fenced_code_block_delimiter")
                    .iter()
                    .map(|d| span(d.range()))
                    .collect();
                let language = n
                    .children()
                    .find(|c| c.kind_name() == "info_string")
                    .and_then(|i| source[i.range()].split_whitespace().next())
                    .unwrap_or("")
                    .to_string();
                out.insert(Fact::Fence {
                    delimiters,
                    language,
                });
            }
            "link_reference_definition" => {
                if let Some(d) = n.children().find(|c| c.kind_name() == "link_destination") {
                    out.insert(Fact::Link {
                        kind: "definition",
                        dest: span(d.range()),
                    });
                }
            }
            "inline" | "pipe_table_cell" => {
                let text = &source[n.range()];
                if !text.contains('[') {
                    continue;
                }
                let Some(links) = inline(text) else {
                    continue;
                };
                for l in links.nodes() {
                    let kind = match l.kind_name() {
                        "inline_link" => "inline",
                        "image" => "image",
                        _ => continue,
                    };
                    if let Some(d) = l.children().find(|c| c.kind_name() == "link_destination") {
                        out.insert(Fact::Link {
                            kind,
                            dest: (n.start() + d.start(), n.start() + d.end()),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn md_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            md_files(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e == "md" || e == "markdown")
        {
            out.push(path);
        }
    }
}

#[test]
fn colab_markdown_reads_what_tree_sitter_md_gave_the_backend() {
    let wide = std::env::var_os("COLAB_MD_FILES");
    let root = wide.clone().map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        PathBuf::from,
    );
    let mut files = Vec::new();
    md_files(&root, &mut files);
    files.sort();
    let block: tree_sitter::Language = tree_sitter_md::LANGUAGE.into();
    let inline_lang: tree_sitter::Language = tree_sitter_md::INLINE_LANGUAGE.into();
    let theirs_inline = |text: &str| colab_rewrite::syntax::tree(&inline_lang, text);
    let ours_inline = |text: &str| colab_parse_markdown::parse_inline(text).ok();
    let show: usize = std::env::var("COLAB_MD_SHOW")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(20);
    let (mut checked, mut same) = (0usize, 0usize);
    let mut lossy = Vec::new();
    let mut differ = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        checked += 1;
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let ours = match colab_parse_markdown::parse(&text) {
            Ok(t) => t,
            Err(e) => {
                lossy.push(format!("{shown}: {e:?}"));
                continue;
            }
        };
        let rebuilt: String = ours.tokens().map(|t| t.text()).collect();
        if rebuilt != text {
            lossy.push(shown);
            continue;
        }
        let Some(theirs) = colab_rewrite::syntax::tree(&block, &text) else {
            continue;
        };
        let (a, b) = (
            facts(&text, &ours, &ours_inline),
            facts(&text, &theirs, &theirs_inline),
        );
        if a == b {
            same += 1;
            continue;
        }
        let line = |at: usize| text[..at.min(text.len())].matches('\n').count() + 1;
        let describe = |f: &Fact| match f {
            Fact::Heading { content: s, .. }
            | Fact::Cell { cell: s, .. }
            | Fact::Item { text: s, .. }
            | Fact::Link { dest: s, .. } => {
                format!("{f:?} l{} {:?}", line(s.0), &text[s.0..s.1])
            }
            Fact::Fence { delimiters, .. } => {
                format!("{f:?} l{}", line(delimiters.first().map_or(0, |d| d.0)))
            }
        };
        let only_ours: Vec<String> = a.difference(&b).take(2).map(describe).collect();
        let only_theirs: Vec<String> = b.difference(&a).take(2).map(describe).collect();
        differ.push(format!(
            "{shown}\n    ours only:   {only_ours:?}\n    theirs only: {only_theirs:?}"
        ));
    }
    eprintln!(
        "{checked} files: {same} read the same, {} differ",
        differ.len()
    );
    for d in differ.iter().take(show) {
        eprintln!("{d}");
    }
    assert!(lossy.is_empty(), "not lossless:\n{}", lossy.join("\n"));
    if wide.is_none() {
        // Where tree-sitter-md misreads CommonMark and colab does not.
        const KNOWN: &[(&str, &str)] = &[(
            "plugins/claude/colab/skills/colab-codemod/SKILL.md",
            "a paragraph line wrapped to start `3) …`: only an ordered list starting at 1 interrupts a paragraph",
        )];
        let unexpected: Vec<&String> = differ
            .iter()
            .filter(|d| !KNOWN.iter().any(|(file, _)| d.starts_with(file)))
            .collect();
        assert!(unexpected.is_empty(), "{} files differ", unexpected.len());
    }
}
