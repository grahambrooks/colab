//! Symbol rename for Go.
//!
//! Visits `identifier`, `type_identifier`, and `field_identifier`
//! nodes whose text equals the target name and rewrites them.
//! Package identifiers (`package foo`, `import "p"`) and label
//! names are deliberately excluded so renaming a function `Foo`
//! never touches a package literal of the same name.
//!
//! This is a *syntactic* rename: scope and shadowing are not
//! analysed. If a local variable in some function happens to share a
//! name with a top-level type, both are renamed. Verify with
//! `--format diff` before applying.
//!
//! The rewrite itself lives in [`colab_rewrite::SourceLanguage::rename`] —
//! all twelve backends share one implementation and differ only in which
//! node kinds count as an identifier.

/// Node kinds this backend treats as renameable identifiers.
pub(crate) const RENAME_KINDS: &[&str] = &["identifier", "type_identifier", "field_identifier"];

#[cfg(test)]
fn rename(from: &str, to: &str, source_code: &str) -> String {
    crate::LANG.rename(from, to, source_code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use colab_core::Operation;
    use std::path::Path;

    #[test]
    fn renaming_onto_a_name_already_in_the_file_is_a_conflict() {
        let op = crate::LANG.symbol_rename("load", "parse");
        let src = "package p\n\nfunc load() {}\n\nfunc parse() {}\n";
        let conflicts = op.conflicts(src, &op.apply(src));
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].contains("(line 5)"), "{conflicts:?}");
        let clean = "package p\n\nfunc load() {}\n";
        assert!(op.conflicts(clean, &op.apply(clean)).is_empty());
    }

    #[test]
    fn renames_function_declaration_and_calls() {
        let src = "package main\n\nfunc Foo() {}\n\nfunc main() { Foo() }\n";
        let out = rename("Foo", "Bar", src);
        assert_eq!(
            out,
            "package main\n\nfunc Bar() {}\n\nfunc main() { Bar() }\n"
        );
    }

    #[test]
    fn renames_struct_type_and_method_receiver() {
        let src = "package main\n\ntype Old struct { X int }\nfunc (o *Old) M() {}\n";
        let out = rename("Old", "New", src);
        assert!(out.contains("type New struct"));
        assert!(out.contains("(o *New)"));
    }

    #[test]
    fn does_not_rename_package_identifier_or_string_literal() {
        // `Foo` appears inside a string and as an import path — must
        // not be renamed.
        let src = "package main\n\nimport \"Foo\"\n\nvar s = \"Foo\"\nfunc main() { _ = s }\n";
        let out = rename("Foo", "Bar", src);
        assert_eq!(out, src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "package main\n\ntype Foo int\nfunc main() { var x Foo = 1; _ = x }\n";
        let once = rename("Foo", "Bar", src);
        let twice = rename("Foo", "Bar", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn returns_input_unchanged_when_symbol_absent() {
        let src = "package main\n\nfunc main() {}\n";
        assert_eq!(rename("Foo", "Bar", src), src);
    }

    #[test]
    fn symbol_rename_operation_is_relevant_for_go_files() {
        let op = crate::LANG.symbol_rename("a", "b");
        assert!(op.is_file_relevant(Path::new("foo.go")));
        assert!(!op.is_file_relevant(Path::new("foo.rs")));
    }
}
