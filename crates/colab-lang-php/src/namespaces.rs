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
use tree_sitter::Node;

const NAMESPACE_KINDS: &[&str] = &["namespace_definition"];

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
        crate::is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.from)
    }
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };
    let mut edits: Vec<(usize, usize)> = Vec::new();
    colab_rewrite::visit_all(&tree, |node| collect(node, source_code, from, &mut edits));
    colab_rewrite::apply_edits(
        source_code,
        edits
            .into_iter()
            .map(|(start, end)| colab_rewrite::Edit::new(start, end, to))
            .collect(),
    )
}

fn collect(node: Node<'_>, source: &str, from: &str, out: &mut Vec<(usize, usize)>) {
    if NAMESPACE_KINDS.contains(&node.kind())
        && let Some(name) = node.child_by_field_name("name")
        && let Ok(text) = name.utf8_text(source.as_bytes())
        && text == from
    {
        out.push((name.start_byte(), name.end_byte()));
    }
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
