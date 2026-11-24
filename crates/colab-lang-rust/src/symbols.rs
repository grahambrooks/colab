//! Symbol rename for Rust.
//!
//! Rewrites every `identifier`, `type_identifier`, `field_identifier`,
//! and `shorthand_field_identifier` node whose text equals the
//! target. Macro names, lifetimes, label names, and tokens that
//! merely look like identifiers (string contents, etc.) are
//! deliberately excluded.
//!
//! Syntactic, not semantic — shadowing and scope are not analysed.
//! Verify with `--format diff` before applying.
//!
//! The rewrite itself lives in [`colab_rewrite::rename_nodes_by_text`] —
//! all twelve backends share one implementation and differ only in which
//! node kinds count as an identifier.

/// Node kinds this backend treats as renameable identifiers.
pub(crate) const RENAME_KINDS: &[&str] = &[
    "identifier",
    "type_identifier",
    "field_identifier",
    "shorthand_field_identifier",
];

#[cfg(test)]
fn rename(from: &str, to: &str, source_code: &str) -> String {
    crate::LANG.rename(from, to, source_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_function_declaration_and_calls() {
        let src = "fn old_fn() {}\n\nfn main() { old_fn(); }\n";
        let out = rename("old_fn", "new_fn", src);
        assert_eq!(out, "fn new_fn() {}\n\nfn main() { new_fn(); }\n");
    }

    #[test]
    fn renames_struct_and_field_uses() {
        let src = "struct Old { x: i32 }\nfn main() { let _ = Old { x: 1 }; }\n";
        let out = rename("Old", "New", src);
        assert!(out.contains("struct New"));
        assert!(out.contains("New { x: 1 }"));
    }

    #[test]
    fn renames_field_identifier() {
        let src = "struct S { old_field: i32 }\nfn main() { let s = S { old_field: 1 }; let _ = s.old_field; }\n";
        let out = rename("old_field", "new_field", src);
        assert!(out.contains("new_field: i32"));
        assert!(out.contains("s.new_field"));
    }

    #[test]
    fn does_not_rename_string_literals() {
        let src = "fn main() { let _ = \"foo\"; }\n";
        assert_eq!(rename("foo", "WRONG", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "fn old_fn() {}\nfn main() { old_fn(); }\n";
        let once = rename("old_fn", "new_fn", src);
        let twice = rename("old_fn", "new_fn", &once);
        assert_eq!(once, twice);
    }
}
