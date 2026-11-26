//! Rename a C# `namespace` declaration.
//!
//! Both forms carry a `name` field and are handled identically:
//!
//! ```csharp
//! namespace Old.Ns { }   // block-scoped
//! namespace Old.Ns;      // file-scoped (C# 10+)
//! ```
//!
//! Matching is exact on the declared name, so `Old` does not match
//! `namespace Old.Ns`. This rewrites the declaration only; qualified
//! uses elsewhere are `csharp::symbol` or `csharp::using` work.

use std::fmt;
use std::path::Path;

use colab_core::Operation;

const NAMESPACE_KINDS: &[&str] = &["namespace_declaration", "file_scoped_namespace_declaration"];

#[derive(Debug)]
pub struct NamespaceRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for NamespaceRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "csharp::namespace \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for NamespaceRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::LANG.is_source(path)
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        names(source)
            .into_iter()
            .map(|(_, text)| text.to_string())
            .collect()
    }
}

/// Every declared namespace name in `source`, with its range.
fn names(source: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let Some(tree) = crate::tree(source) else {
        return Vec::new();
    };
    tree.nodes()
        .filter(|n| n.is_named() && NAMESPACE_KINDS.contains(&n.kind_name()))
        .filter_map(|n| n.child_by_field("name"))
        .map(|name| (name.range(), &source[name.range()]))
        .collect()
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let edits = names(source_code)
        .into_iter()
        .filter(|(_, text)| *text == from)
        .map(|(range, _)| colab_rewrite::Edit::new(range.start, range.end, to))
        .collect();
    colab_rewrite::apply_edits(source_code, edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_a_block_scoped_namespace() {
        let src = "namespace Old.Ns { class C { } }\n";
        let out = rename("Old.Ns", "New.Ns", src);
        assert!(out.contains("namespace New.Ns {"), "got: {out}");
    }

    #[test]
    fn renames_a_file_scoped_namespace() {
        let src = "namespace Old.Ns;\n\nclass C { }\n";
        let out = rename("Old.Ns", "New.Ns", src);
        assert!(out.contains("namespace New.Ns;"), "got: {out}");
    }

    #[test]
    fn does_not_match_a_prefix_of_the_declared_name() {
        let src = "namespace Old.Ns { }\n";
        assert_eq!(rename("Old", "WRONG", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "namespace Old.Ns;\n";
        let once = rename("Old.Ns", "New.Ns", src);
        let twice = rename("Old.Ns", "New.Ns", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "namespace Other { }\n";
        assert_eq!(rename("Old.Ns", "New.Ns", src), src);
    }
}
