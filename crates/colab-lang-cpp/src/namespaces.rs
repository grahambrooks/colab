//! Rename a C++ `namespace` declaration.
//!
//! `namespace_definition` carries a `name` field that is either a
//! `namespace_identifier` (`namespace foo {`) or a
//! `nested_namespace_specifier` (`namespace foo::bar {`). An anonymous
//! namespace has no `name` field at all and never matches.
//!
//! Matching is exact on the declared name, so `foo` matches
//! `namespace foo` but not `namespace foo::bar` — target the nested form
//! verbatim (`"foo::bar"`) when that is what you mean.
//!
//! This rewrites the *declaration* only. Qualified uses of the namespace
//! elsewhere in the file (`foo::thing`) are `namespace_identifier` nodes
//! that `cpp::symbol` covers, so a full rename is usually two rules.

use std::fmt;
use std::path::Path;

use colab_core::Operation;

#[derive(Debug)]
pub struct NamespaceRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for NamespaceRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cpp::namespace \"{}\" -> \"{}\"", self.from, self.to)
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

/// Every namespace definition's name in `source`, with its range.
fn names(source: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let Some(tree) = crate::tree(source) else {
        return Vec::new();
    };
    tree.nodes()
        .filter(|n| n.is_named() && n.kind_name() == "namespace_definition")
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
    fn renames_a_simple_namespace() {
        let src = "namespace old_ns {\nvoid f() {}\n}\n";
        let out = rename("old_ns", "new_ns", src);
        assert!(out.contains("namespace new_ns {"), "got: {out}");
    }

    #[test]
    fn renames_a_nested_namespace_specifier_verbatim() {
        let src = "namespace a::b {\n}\n";
        let out = rename("a::b", "c::d", src);
        assert!(out.contains("namespace c::d {"), "got: {out}");
    }

    #[test]
    fn a_simple_target_does_not_match_a_nested_declaration() {
        let src = "namespace a::b {\n}\n";
        assert_eq!(rename("a", "WRONG", src), src);
    }

    #[test]
    fn anonymous_namespaces_never_match() {
        let src = "namespace {\nvoid f() {}\n}\n";
        assert_eq!(rename("old_ns", "new_ns", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "namespace old_ns {\n}\n";
        let once = rename("old_ns", "new_ns", src);
        let twice = rename("old_ns", "new_ns", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "namespace other {\n}\n";
        assert_eq!(rename("old_ns", "new_ns", src), src);
    }
}
