//! Broken links: a relative link whose file does not exist, or whose
//! `#anchor` names no heading in its target.
//!
//! The files are seen through [`Files`], so a caller can check the tree as
//! a run *would* leave it, before writing anything, and compare that with
//! the tree as it is. Links with a scheme (`https:`, `mailto:`), absolute
//! paths and anchors into non-Markdown files are not checked: they cannot
//! be resolved from the files alone.

use std::path::{Component, Path, PathBuf};

use crate::link::links;
use crate::outline;

/// How a link check sees the files.
pub trait Files {
    /// The file's text, when it is a file colab can read.
    fn read(&self, path: &Path) -> Option<String>;
    /// Whether anything (a file or a directory) is at `path`.
    fn exists(&self, path: &Path) -> bool;
}

/// One broken link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broken {
    pub line: usize,
    pub dest: String,
    pub reason: String,
}

/// Parsed anchors by file, shared by every [`broken_links_cached`] call
/// over one view of the files, so each target is parsed once per run.
#[derive(Default)]
pub struct AnchorCache(std::collections::HashMap<PathBuf, Option<Anchors>>);

/// Every broken link in `source`, the text of `path`.
pub fn broken_links(path: &Path, source: &str, files: &dyn Files) -> Vec<Broken> {
    broken_links_cached(path, source, files, &mut AnchorCache::default())
}

/// [`broken_links`], parsing each target's anchors at most once across
/// calls that share `cache` (and so must share one view of the files).
pub fn broken_links_cached(
    path: &Path,
    source: &str,
    files: &dyn Files,
    cache: &mut AnchorCache,
) -> Vec<Broken> {
    let dir = path.parent().unwrap_or(Path::new(""));
    let mut out = Vec::new();
    let anchors = &mut cache.0;
    let mut own: Option<Option<Anchors>> = None;
    for link in links(source) {
        let dest = link.dest.trim_start_matches('<').trim_end_matches('>');
        if dest.is_empty() || has_scheme(dest) || dest.starts_with('/') {
            continue;
        }
        let (file, anchor) = match dest.find('#') {
            Some(i) => (&dest[..i], Some(&dest[i + 1..])),
            None => (dest, None),
        };
        let target = if file.is_empty() {
            path.to_path_buf()
        } else {
            normalise(&dir.join(decode(file)))
        };
        if !file.is_empty() && !files.exists(&target) {
            out.push(Broken {
                line: link.line,
                dest: link.dest.clone(),
                reason: format!("no file `{}`", target.display()),
            });
            continue;
        }
        let Some(anchor) = anchor.filter(|a| !a.is_empty()) else {
            continue;
        };
        if !crate::is_markdown(&target) {
            continue;
        }
        // The file's own anchors come from `source`, which may differ from
        // what `files` holds for it: parsed once per call, not cached.
        let anchors = if file.is_empty() {
            &*own.get_or_insert_with(|| Some(Anchors::of(source)))
        } else {
            &*anchors
                .entry(target.clone())
                .or_insert_with(|| files.read(&target).map(|t| Anchors::of(&t)))
        };
        let Some(anchors) = anchors else { continue };
        if !anchors.has(anchor) {
            out.push(Broken {
                line: link.line,
                dest: link.dest.clone(),
                reason: if file.is_empty() {
                    format!("no section `#{anchor}` in this file")
                } else {
                    format!("no section `#{anchor}` in `{}`", target.display())
                },
            });
        }
    }
    out
}

/// What a file's anchors are: its heading slugs, and its text for HTML
/// `id` / `name` attributes.
pub struct Anchors {
    slugs: std::collections::HashSet<String>,
    lower: String,
}

impl Anchors {
    fn of(text: &str) -> Self {
        Self {
            slugs: outline::headings(text)
                .into_iter()
                .map(|h| h.slug)
                .collect(),
            lower: text.to_lowercase(),
        }
    }

    fn has(&self, anchor: &str) -> bool {
        let anchor = decode(anchor).to_lowercase();
        self.slugs.contains(&anchor)
            || [format!("id=\"{anchor}\""), format!("name=\"{anchor}\"")]
                .iter()
                .any(|attr| self.lower.contains(attr.as_str()))
    }
}

fn has_scheme(dest: &str) -> bool {
    dest.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// `%20` and friends.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&text[i + 1..(i + 3).min(text.len())], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// `a/./b/../c` → `a/c`, without touching the filesystem.
pub fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Fake(HashMap<PathBuf, String>);

    impl Files for Fake {
        fn read(&self, path: &Path) -> Option<String> {
            self.0.get(path).cloned()
        }
        fn exists(&self, path: &Path) -> bool {
            self.0.contains_key(path) || path == Path::new("docs")
        }
    }

    #[test]
    fn finds_missing_files_and_anchors_and_skips_what_it_cannot_resolve() {
        let files = Fake(HashMap::from([
            (
                PathBuf::from("docs/guide.md"),
                "# Guide\n\n## Usage\n".to_string(),
            ),
            (PathBuf::from("img.png"), String::new()),
        ]));
        let source = "# Home\n\n[a](docs/guide.md#usage) [b](docs/guide.md#install) [c](docs/gone.md)\n[d](#home) [e](#nope) [f](https://x.dev/#y) [g](../img.png) [h](docs) [i](/abs.md)\n";
        let broken = broken_links(Path::new("sub/README.md"), source, &files);
        let shown: Vec<&str> = broken.iter().map(|b| b.dest.as_str()).collect();
        // From sub/, docs/guide.md does not exist; ../img.png does.
        assert_eq!(
            shown,
            [
                "docs/guide.md#usage",
                "docs/guide.md#install",
                "docs/gone.md",
                "#nope",
                "docs"
            ]
        );
        let top = broken_links(Path::new("README.md"), source, &files);
        let shown: Vec<&str> = top.iter().map(|b| b.dest.as_str()).collect();
        assert_eq!(
            shown,
            [
                "docs/guide.md#install",
                "docs/gone.md",
                "#nope",
                "../img.png"
            ]
        );
        assert!(
            top[0]
                .reason
                .contains("no section `#install` in `docs/guide.md`")
        );
    }
}
