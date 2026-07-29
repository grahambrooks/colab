//! Symbol rename for C++.
//!
//! Rewrites identifier-kind nodes whose text equals the target. This is
//! syntactic, not semantic: scope and shadowing are not analysed, so a
//! local that happens to share a name with a type is renamed too. Narrow
//! a rule with `in "<glob>"` when a name is not unique across the tree.
//!
//! The rewrite itself lives in [`colab_rewrite::rename_nodes_by_text`] —
//! all twelve backends share one implementation and differ only in which
//! node kinds count as an identifier.

use std::fmt;
use std::path::Path;

use colab_core::Operation;

/// Node kinds this backend treats as renameable identifiers.
const RENAME_KINDS: &[&str] = &["identifier", "type_identifier", "field_identifier", "namespace_identifier"];

#[derive(Debug)]
pub struct SymbolRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for SymbolRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cpp::symbol \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for SymbolRename {
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
    colab_rewrite::rename_nodes_by_text(&tree, source_code, RENAME_KINDS, from, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "class Old {};\nOld make() { Old o; return o; }\n";

    #[test]
    fn renames_declaration_and_usages() {
        let out = rename("Old", "New", SRC);
        assert!(out.contains("New"), "got: {out}");
        assert!(!out.contains("Old"), "got: {out}");
    }

    #[test]
    fn does_not_rename_inside_a_string_literal() {
        let src = SRC.replace("Old", "New");
        // Nothing named `Old` remains, so this must be a no-op.
        assert_eq!(rename("Old", "OTHER", &src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let once = rename("Old", "New", SRC);
        let twice = rename("Old", "New", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        assert_eq!(rename("NotPresentAnywhere", "X", SRC), SRC);
    }
}
