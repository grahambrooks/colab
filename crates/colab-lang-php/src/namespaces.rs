//! Rename a PHP `namespace` declaration.
//!
//! `namespace_definition` carries a `name` field holding the
//! backslash-separated namespace name. Both the statement form
//! (`namespace App\Old;`) and the block form (`namespace App\Old { }`)
//! are handled.
//!
//! Matching is exact on the declared name, so `App` does not match
//! `namespace App\Old`. This rewrites the declaration only; `use`
//! statements referring to the namespace are `php::use` work.

use std::fmt;
use std::path::Path;

use colab_core::Operation;

/// Each `namespace_definition`'s name node's range.
fn declared_names(source: &str) -> Vec<std::ops::Range<usize>> {
    let Some(tree) = crate::tree(source) else {
        return Vec::new();
    };
    tree.nodes()
        .filter(|n| {
            n.is_named() && n.kind() == const { colab_parse_php::kind("namespace_definition") }
        })
        .filter_map(|n| n.child_by_field("name"))
        .map(|n| n.range())
        .collect()
}

#[derive(Debug)]
pub struct NamespaceRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for NamespaceRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "php::namespace \"{}\" -> \"{}\"", self.from, self.to)
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
        declared_names(source)
            .into_iter()
            .map(|r| source[r].to_string())
            .collect()
    }
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let edits = declared_names(source_code)
        .into_iter()
        .filter(|r| &source_code[r.clone()] == from)
        .map(|r| colab_rewrite::Edit::new(r.start, r.end, to))
        .collect();
    colab_rewrite::apply_edits(source_code, edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_a_statement_form_namespace() {
        let src = "<?php\nnamespace App\\Old;\n\nclass C {}\n";
        let out = rename("App\\Old", "App\\New", src);
        assert!(out.contains("namespace App\\New;"), "got: {out}");
    }

    #[test]
    fn renames_a_block_form_namespace() {
        let src = "<?php\nnamespace App\\Old { }\n";
        let out = rename("App\\Old", "App\\New", src);
        assert!(out.contains("namespace App\\New {"), "got: {out}");
    }

    #[test]
    fn does_not_match_a_prefix_of_the_declared_name() {
        let src = "<?php\nnamespace App\\Old;\n";
        assert_eq!(rename("App", "WRONG", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "<?php\nnamespace App\\Old;\n";
        let once = rename("App\\Old", "App\\New", src);
        let twice = rename("App\\Old", "App\\New", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "<?php\nnamespace Other;\n";
        assert_eq!(rename("App\\Old", "App\\New", src), src);
    }
}
