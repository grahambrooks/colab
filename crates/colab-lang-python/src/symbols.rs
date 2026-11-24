//! Symbol rename for Python.
//!
//! Rewrites every `identifier` node whose text equals the target.
//! Syntactic, not semantic — local shadows of a top-level name are
//! also renamed. Verify with `--format diff` before applying.
//!
//! The rewrite itself lives in [`colab_rewrite::rename_nodes_by_text`] —
//! all twelve backends share one implementation and differ only in which
//! node kinds count as an identifier.

/// Node kinds this backend treats as renameable identifiers.
pub(crate) const RENAME_KINDS: &[&str] = &["identifier"];

#[cfg(test)]
fn rename(from: &str, to: &str, source_code: &str) -> String {
    crate::LANG.rename(from, to, source_code)
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
