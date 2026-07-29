//! Rename a Kotlin `package` header.
//!
//! `package_header` holds a `qualified_identifier` with the dotted
//! package name. Matching is exact, so `com.old` does not match
//! `package com.old.sub`.
//!
//! This rewrites the declaration only. Imports naming the package are
//! `kotlin::import` work — a package move is usually both rules in one
//! script.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::Node;

#[derive(Debug)]
pub struct PackageRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for PackageRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "kotlin::package \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for PackageRename {
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
    if node.kind() == "package_header" {
        for i in 0..node.named_child_count() {
            let Some(child) = node.named_child(i as u32) else {
                break;
            };
            if child.kind() == "qualified_identifier"
                && let Ok(text) = child.utf8_text(source.as_bytes())
                && text == from
            {
                out.push((child.start_byte(), child.end_byte()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_the_package_header() {
        let src = "package com.old.pkg\n\nfun main() {}\n";
        let out = rename("com.old.pkg", "com.new.pkg", src);
        assert!(out.contains("package com.new.pkg"), "got: {out}");
    }

    #[test]
    fn does_not_match_a_prefix() {
        let src = "package com.old.pkg\n";
        assert_eq!(rename("com.old", "WRONG", src), src);
    }

    #[test]
    fn does_not_touch_imports() {
        let src = "package com.demo\n\nimport com.old.pkg.Thing\n";
        assert_eq!(rename("com.old.pkg.Thing", "X", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "package com.old.pkg\n";
        let once = rename("com.old.pkg", "com.new.pkg", src);
        let twice = rename("com.old.pkg", "com.new.pkg", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn absent_target_returns_input_unchanged() {
        let src = "package com.other\n";
        assert_eq!(rename("com.old.pkg", "com.new.pkg", src), src);
    }
}
