//! Rename the `package` declaration in a Java source file.
//!
//! The matching is exact-equality on the dotted package name. Files
//! whose package does not equal `from` are left untouched.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Parser, Tree};

fn parse_java(source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .expect("failed to load tree-sitter Java grammar");
    parser.parse(source, None)
}

#[derive(Debug)]
pub struct PackageRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for PackageRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "java::package \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for PackageRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("java")
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }
}

pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    let Some(tree) = parse_java(source_code) else {
        return source_code.to_string();
    };
    let root = tree.root_node();
    for i in 0..root.named_child_count() {
        let Some(child) = root.named_child(i) else {
            break;
        };
        if child.kind() != "package_declaration" {
            continue;
        }
        // Find the dotted name child.
        for j in 0..child.named_child_count() {
            let Some(name_node) = child.named_child(j) else {
                break;
            };
            if !matches!(
                name_node.kind(),
                "scoped_identifier" | "identifier"
            ) {
                continue;
            }
            let Ok(name) = name_node.utf8_text(source_code.as_bytes()) else {
                continue;
            };
            if name == from {
                let mut out = source_code.to_string();
                out.replace_range(
                    name_node.start_byte()..name_node.end_byte(),
                    to,
                );
                return out;
            }
        }
    }
    source_code.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_matching_package() {
        let src = "package com.old;\n\npublic class Hello {}\n";
        let out = rename("com.old", "com.new", src);
        assert!(out.starts_with("package com.new;"), "got: {out}");
    }

    #[test]
    fn does_not_rename_mismatched_package() {
        let src = "package com.other;\n";
        assert_eq!(rename("com.old", "com.new", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "package com.old;\n";
        let once = rename("com.old", "com.new", src);
        let twice = rename("com.old", "com.new", &once);
        assert_eq!(once, twice);
    }
}
