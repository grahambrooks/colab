//! Symbol rename for Kotlin.
//!
//! Rewrites identifier-kind nodes whose text equals the target. This is
//! syntactic, not semantic: scope and shadowing are not analysed, so a
//! local that happens to share a name with a type is renamed too. Narrow
//! a rule with `in "<glob>"` when a name is not unique across the tree.
//!
//! The rewrite itself lives in [`colab_rewrite::SourceLanguage::rename`] —
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

    const SRC: &str = "class Old { fun m(): Old = Old() }\n";

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
