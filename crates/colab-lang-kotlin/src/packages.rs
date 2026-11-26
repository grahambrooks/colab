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
        crate::LANG.is_source(path)
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn near_miss_target(&self) -> Option<&str> {
        Some(&self.from)
    }

    fn observed_targets(&self, source: &str) -> Vec<String> {
        package_names(source)
    }
}

/// Each `package` header's name: its range.
fn package_ranges(source: &str) -> Vec<std::ops::Range<usize>> {
    let Some(tree) = crate::tree(source) else {
        return Vec::new();
    };
    tree.nodes()
        .filter(|n| n.is_named() && n.kind_name() == "package_header")
        .flat_map(|n| {
            n.named_children()
                .filter(|c| c.kind_name() == "qualified_identifier")
                .map(|c| c.range())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The name in every `package` header in `source`, for near-miss reporting.
fn package_names(source: &str) -> Vec<String> {
    package_ranges(source)
        .into_iter()
        .map(|r| source[r].to_string())
        .collect()
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let edits = package_ranges(source_code)
        .into_iter()
        .filter(|r| &source_code[r.clone()] == from)
        .map(|r| colab_rewrite::Edit::new(r.start, r.end, to))
        .collect();
    colab_rewrite::apply_edits(source_code, edits)
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
