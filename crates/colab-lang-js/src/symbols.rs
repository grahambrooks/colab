//! Symbol rename for JavaScript / TypeScript.
//!
//! Rewrites `identifier`, `property_identifier`, and
//! `shorthand_property_identifier` nodes whose text equals the
//! target. Syntactic, not semantic — JSX attribute names that share
//! a function name will also be touched. Verify with
//! `--format diff`.
//!
//! The rewrite itself lives in [`colab_rewrite::rename_nodes_by_text`] —
//! all twelve backends share one implementation and differ only in which
//! node kinds count as an identifier.

/// Node kinds this backend treats as renameable identifiers.
pub(crate) const RENAME_KINDS: &[&str] = &[
    "identifier",
    "property_identifier",
    "shorthand_property_identifier",
];

#[cfg(test)]
fn rename(from: &str, to: &str, source_code: &str) -> String {
    crate::LANG.rename(from, to, source_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_function_decl_and_calls() {
        let src = "function oldFn() {}\noldFn();\n";
        let out = rename("oldFn", "newFn", src);
        assert_eq!(out, "function newFn() {}\nnewFn();\n");
    }

    #[test]
    fn renames_object_property_access() {
        let src = "const x = obj.oldProp;\n";
        let out = rename("oldProp", "newProp", src);
        assert_eq!(out, "const x = obj.newProp;\n");
    }

    #[test]
    fn does_not_rename_string_literal() {
        let src = "const x = \"oldFn\";\n";
        assert_eq!(rename("oldFn", "newFn", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "function oldFn() {}\noldFn();\n";
        let once = rename("oldFn", "newFn", src);
        let twice = rename("oldFn", "newFn", &once);
        assert_eq!(once, twice);
    }
}
