//! Operations on Kotlin `import` directives: rename, delete, ensure.
//!
//! An `import` node holds a `qualified_identifier` naming the imported
//! symbol, optionally followed by an alias identifier
//! (`import a.b.C as D`). The target is always the qualified name, never
//! the alias — renaming an alias is `kotlin::symbol` work.
//!
//! Star imports (`import a.b.*`) carry only the package prefix in the
//! name node — the `*` is punctuation. So `import a.b.*` is matched by
//! the target `"a.b"`, and renaming it rewrites the prefix while leaving
//! the `.*` in place. Note this means `"a.b"` matches the star import but
//! not `import a.b.C`, which is its own distinct target.

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use tree_sitter::{Node, Tree};

/// The qualified-name node of an `import`, plus its text.
fn import_name<'a>(node: Node<'a>, source: &'a str) -> Option<(Node<'a>, &'a str)> {
    if node.kind() != "import" {
        return None;
    }
    // The first qualified_identifier is the imported path; a trailing
    // bare identifier, if present, is the alias.
    for i in 0..node.named_child_count() {
        let Some(child) = node.named_child(i as u32) else {
            break;
        };
        if child.kind() == "qualified_identifier"
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
        write!(f, "kotlin::import \"{}\" -> \"{}\"", self.from, self.to)
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
        write!(f, "kotlin::import \"{}\" -> delete", self.target)
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
        write!(f, "kotlin::import \"{}\" -> ensure", self.target)
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

/// Insert `import <target>` after the last existing import, else after
/// the `package` header, else at the top of the file.
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
    let (mut after_import, mut after_package) = (None, None);
    for i in 0..root.named_child_count() {
        let Some(child) = root.named_child(i as u32) else {
            break;
        };
        let end = child.end_byte();
        let line_end = source_code[end..]
            .find('\n')
            .map(|p| end + p + 1)
            .unwrap_or(source_code.len());
        match child.kind() {
            "import" => after_import = Some(line_end),
            "package_header" => after_package = after_package.or(Some(line_end)),
            _ => {}
        }
    }

    let pos = after_import.or(after_package).unwrap_or(0);
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

    const SRC: &str =
        "package com.demo\n\nimport com.old.Thing\nimport com.other.Other\n\nfun main() {}\n";

    #[test]
    fn renames_an_import() {
        let out = rename("com.old.Thing", "com.new.Thing", SRC);
        assert!(out.contains("import com.new.Thing"), "got: {out}");
        assert!(out.contains("import com.other.Other"), "others untouched");
    }

    #[test]
    fn aliased_imports_target_the_qualified_name() {
        let src = "import com.old.Thing as T\n";
        let out = rename("com.old.Thing", "com.new.Thing", src);
        assert_eq!(out, "import com.new.Thing as T\n");
        // The alias itself is not a target here.
        assert_eq!(rename("T", "U", src), src);
    }

    #[test]
    fn star_imports_are_matched_by_their_package_prefix() {
        // The `*` is punctuation, not part of the name node.
        let src = "import com.old.*\n";
        assert_eq!(rename("com.old", "com.new", src), "import com.new.*\n");
        // Writing the star in the target matches nothing.
        assert_eq!(rename("com.old.*", "com.new.*", src), src);
    }

    #[test]
    fn a_package_prefix_does_not_match_a_specific_import() {
        let src = "import com.old.Thing\n";
        assert_eq!(rename("com.old", "com.new", src), src);
    }

    #[test]
    fn rename_does_not_match_a_prefix() {
        assert_eq!(rename("com.old", "WRONG", SRC), SRC);
    }

    #[test]
    fn rename_is_idempotent() {
        let once = rename("com.old.Thing", "com.new.Thing", SRC);
        let twice = rename("com.old.Thing", "com.new.Thing", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn deletes_the_matched_line() {
        let out = delete("com.old.Thing", SRC);
        assert!(!out.contains("com.old.Thing"));
        assert!(out.contains("import com.other.Other"));
        assert!(out.contains("fun main"));
    }

    #[test]
    fn delete_is_idempotent() {
        let once = delete("com.old.Thing", SRC);
        let twice = delete("com.old.Thing", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn ensure_adds_after_the_existing_imports() {
        let out = ensure("com.extra.Extra", SRC);
        assert!(out.contains("import com.extra.Extra"), "got: {out}");
        let last_existing = out.find("import com.other.Other").unwrap();
        let added = out.find("import com.extra.Extra").unwrap();
        assert!(last_existing < added, "got: {out}");
    }

    #[test]
    fn ensure_falls_back_to_after_the_package_header() {
        let src = "package com.demo\n\nfun main() {}\n";
        let out = ensure("com.extra.Extra", src);
        let pkg = out.find("package com.demo").unwrap();
        let added = out.find("import com.extra.Extra").unwrap();
        let func = out.find("fun main").unwrap();
        assert!(pkg < added && added < func, "got: {out}");
    }

    #[test]
    fn ensure_is_a_noop_when_present() {
        assert_eq!(ensure("com.old.Thing", SRC), SRC);
    }

    #[test]
    fn ensure_is_idempotent() {
        let once = ensure("com.extra.Extra", SRC);
        let twice = ensure("com.extra.Extra", &once);
        assert_eq!(once, twice);
    }
}
