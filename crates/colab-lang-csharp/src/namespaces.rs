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
use tree_sitter::Node;

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
