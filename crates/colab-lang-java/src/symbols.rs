//! Symbol rename for Java.
//!
//! Rewrites `identifier` and `type_identifier` nodes whose text
//! equals the target. Syntactic, not semantic — shadowing is not
//! analysed.
//!
//! The rewrite itself lives in [`colab_rewrite::rename_nodes_by_text`] —
//! all twelve backends share one implementation and differ only in which
//! node kinds count as an identifier.

/// Node kinds this backend treats as renameable identifiers.
pub(crate) const RENAME_KINDS: &[&str] = &["identifier", "type_identifier"];

#[cfg(test)]
fn rename(from: &str, to: &str, source_code: &str) -> String {
    crate::LANG.rename(from, to, source_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_class_and_constructor_call() {
        let src = "package demo;\n\npublic class OldName {\n    public OldName() {}\n}\n\nclass User { OldName x = new OldName(); }\n";
        let out = rename("OldName", "NewName", src);
        assert!(out.contains("public class NewName"));
        assert!(out.contains("public NewName()"));
        assert!(out.contains("NewName x = new NewName();"));
    }

    #[test]
    fn renames_method_and_calls() {
        let src = "class A { void oldM() {} void caller() { oldM(); } }\n";
        let out = rename("oldM", "newM", src);
        assert_eq!(
            out,
            "class A { void newM() {} void caller() { newM(); } }\n"
        );
    }

    #[test]
    fn does_not_rename_string_literal() {
        let src = "class A { String s = \"OldName\"; }\n";
        assert_eq!(rename("OldName", "NewName", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "class A { void oldM() {} void caller() { oldM(); } }\n";
        let once = rename("oldM", "newM", src);
        let twice = rename("oldM", "newM", &once);
        assert_eq!(once, twice);
    }
}
