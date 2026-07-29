//! Symbol rename for Python.
//!
//! Rewrites every `identifier` node whose text equals the target.
//! Syntactic, not semantic — local shadows of a top-level name are
//! also renamed. Verify with `--format diff` before applying.
//!
//! The rewrite itself lives in [`colab_rewrite::rename_nodes_by_text`] —
//! all twelve backends share one implementation and differ only in which
//! node kinds count as an identifier.

use std::fmt;
use std::path::Path;

use colab_core::Operation;

/// Node kinds this backend treats as renameable identifiers.
const RENAME_KINDS: &[&str] = &["identifier"];

#[derive(Debug)]
pub struct SymbolRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for SymbolRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "python::symbol \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for SymbolRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("py")
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

    #[test]
    fn renames_function_def_and_calls() {
        let src = "def old_fn():\n    return 1\n\nprint(old_fn())\n";
        let out = rename("old_fn", "new_fn", src);
        assert_eq!(out, "def new_fn():\n    return 1\n\nprint(new_fn())\n");
    }

    #[test]
    fn renames_class_definition_and_use() {
        let src = "class Old:\n    pass\n\nx = Old()\n";
        let out = rename("Old", "New", src);
        assert_eq!(out, "class New:\n    pass\n\nx = New()\n");
    }

    #[test]
    fn does_not_rename_string_or_comment() {
        let src = "x = 'old_fn'\n# old_fn comment\n";
        assert_eq!(rename("old_fn", "new_fn", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "def old_fn(): return 1\nold_fn()\n";
        let once = rename("old_fn", "new_fn", src);
        let twice = rename("old_fn", "new_fn", &once);
        assert_eq!(once, twice);
    }
}
