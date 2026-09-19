//! Operations on Swift `import` declarations: rename, delete, ensure.
//!
//! An `import_declaration` holds an `identifier` node whose text is the
//! module path — `Foundation`, or `UIKit.UIView` for a submodule import.
//! The kind-qualified form (`import class Old.Thing`) has the same shape;
//! the kind keyword is punctuation and is preserved by a rename.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Tree};

/// The module-path node of an `import_declaration`, plus its text.
fn import_name<'a>(node: Node<'a>, source: &'a str) -> Option<(Node<'a>, &'a str)> {
    if node.kind() != "import_declaration" {
        return None;
    }
    for i in 0..node.named_child_count() {
        let Some(child) = node.named_child(i as u32) else {
            break;
        };
        if child.kind() == "identifier"
            && let Ok(text) = child.utf8_text(source.as_bytes())
        {
            return Some((child, text));
        }
    }
    None
}

fn for_each_matching_import<F>(tree: &Tree, source: &str, target: &str, mut visit: F)
where
    F: FnMut(Node<'_>, Node<'_>),
{
    colab_rewrite::visit_all(tree, |node| {
        if let Some((name_node, text)) = import_name(node, source)
            && text == target
        {
            visit(node, name_node);
        }
    });
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ImportRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for ImportRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "swift::import \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for ImportRename {
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
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for_each_matching_import(&tree, source_code, from, |_import, name_node| {
        edits.push((name_node.start_byte(), name_node.end_byte(), to.to_string()));
    });
    colab_rewrite::apply_edits(
        source_code,
        edits
            .into_iter()
            .map(|(start, end, text)| colab_rewrite::Edit::new(start, end, text))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ImportDelete {
    pub target: String,
}

impl fmt::Display for ImportDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "swift::import \"{}\" -> delete", self.target)
    }
}

impl Operation for ImportDelete {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        delete(&self.target, source_code)
    }

    fn prefilter(&self) -> Option<&str> {
        Some(&self.target)
    }
}

pub fn delete(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for_each_matching_import(&tree, source_code, target, |import, _name| {
        // `delete_lines` widens each range to whole lines.
        spans.push((import.start_byte(), import.end_byte()));
    });
    colab_rewrite::delete_lines(source_code, spans)
}

// ---------------------------------------------------------------------------
// Ensure
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ImportEnsure {
    pub target: String,
}

impl fmt::Display for ImportEnsure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "swift::import \"{}\" -> ensure", self.target)
    }
}

impl Operation for ImportEnsure {
    fn is_file_relevant(&self, path: &Path) -> bool {
        crate::is_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        ensure(&self.target, source_code)
    }

    // No prefilter: `ensure` acts precisely when the target is absent.
}

/// Insert `import <target>` after the last existing import, else at the
/// top of the file.
pub fn ensure(target: &str, source_code: &str) -> String {
    let Some(tree) = crate::parse(source_code) else {
        return source_code.to_string();
    };

    let mut already_present = false;
    for_each_matching_import(&tree, source_code, target, |_, _| {
        already_present = true;
    });
    if already_present {
        return source_code.to_string();
    }

    let root = tree.root_node();
    let mut after_import = None;
    for i in 0..root.named_child_count() {
        let Some(child) = root.named_child(i as u32) else {
            break;
        };
        let end = child.end_byte();
        let line_end = source_code[end..]
            .find('\n')
            .map(|p| end + p + 1)
            .unwrap_or(source_code.len());
        if child.kind() == "import_declaration" {
            after_import = Some(line_end);
        }
    }

    let pos = after_import.unwrap_or(0);
    let insertion = format!("import {}\n", target);
    let mut out = String::with_capacity(source_code.len() + insertion.len());
    out.push_str(&source_code[..pos]);
    out.push_str(&insertion);
    out.push_str(&source_code[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "import Foundation\nimport OldModule\n\nfunc main() {}\n";

    #[test]
    fn renames_an_import() {
        let out = rename("OldModule", "NewModule", SRC);
        assert!(out.contains("import NewModule"), "got: {out}");
        assert!(out.contains("import Foundation"), "others untouched");
    }

    #[test]
    fn renames_a_submodule_import() {
        let src = "import UIKit.UIView\n";
        assert_eq!(
            rename("UIKit.UIView", "UIKit.UILabel", src),
            "import UIKit.UILabel\n"
        );
    }

    #[test]
    fn a_kind_qualified_import_keeps_its_keyword() {
        let src = "import class Old.Thing\n";
        assert_eq!(
            rename("Old.Thing", "New.Thing", src),
            "import class New.Thing\n"
        );
    }

    #[test]
    fn rename_does_not_match_a_prefix() {
        let src = "import UIKit.UIView\n";
        assert_eq!(rename("UIKit", "WRONG", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let once = rename("OldModule", "NewModule", SRC);
        let twice = rename("OldModule", "NewModule", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn deletes_the_matched_line() {
        let out = delete("OldModule", SRC);
        assert!(!out.contains("OldModule"));
        assert!(out.contains("import Foundation"));
        assert!(out.contains("func main"));
    }

    #[test]
    fn delete_is_idempotent() {
        let once = delete("OldModule", SRC);
        let twice = delete("OldModule", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn ensure_adds_after_the_existing_imports() {
        let out = ensure("Combine", SRC);
        assert!(out.contains("import Combine"), "got: {out}");
        let last_existing = out.find("import OldModule").unwrap();
        let added = out.find("import Combine").unwrap();
        assert!(last_existing < added, "got: {out}");
    }

    #[test]
    fn ensure_prepends_when_there_are_no_imports() {
        let src = "func main() {}\n";
        let out = ensure("Foundation", src);
        assert!(out.starts_with("import Foundation\n"), "got: {out}");
    }

    #[test]
    fn ensure_is_a_noop_when_present() {
        assert_eq!(ensure("Foundation", SRC), SRC);
    }

    #[test]
    fn ensure_is_idempotent() {
        let once = ensure("Combine", SRC);
        let twice = ensure("Combine", &once);
        assert_eq!(once, twice);
    }
}
