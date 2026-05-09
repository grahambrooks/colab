//! Rename dependencies in `Cargo.toml`.
//!
//! Edits the keys in `[dependencies]`, `[dev-dependencies]`, and
//! `[build-dependencies]` (plus their dotted-table forms like
//! `[dependencies.foo]`).
//!
//! Implementation note: we parse the document with `toml_edit` first
//! to validate it and to confirm the dependency actually exists in a
//! supported table — so we never rewrite a string that just happens
//! to look like a key. Once validated, we scan the source line-by-line
//! and substitute the key in place. The line scan preserves position,
//! whitespace, and comments exactly, which `toml_edit::Table::remove`
//! + `Table::insert` would not (it moves the renamed key to the end).

use std::fmt;
use std::path::Path;

use colab_core::Operation;
use toml_edit::DocumentMut;

const DEP_TABLES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

/// `Operation` that renames a Cargo.toml dependency.
#[derive(Debug)]
pub struct CrateRename {
    pub from: String,
    pub to: String,
}

impl fmt::Display for CrateRename {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rust::crate \"{}\" -> \"{}\"", self.from, self.to)
    }
}

impl Operation for CrateRename {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.file_name().and_then(|s| s.to_str()) == Some("Cargo.toml")
    }

    fn apply(&self, source_code: &str) -> String {
        rename(&self.from, &self.to, source_code)
    }
}

/// `Operation` that removes a Cargo.toml dependency entry.
#[derive(Debug)]
pub struct CrateDelete {
    pub target: String,
}

impl fmt::Display for CrateDelete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rust::crate \"{}\" -> delete", self.target)
    }
}

impl Operation for CrateDelete {
    fn is_file_relevant(&self, path: &Path) -> bool {
        path.file_name().and_then(|s| s.to_str()) == Some("Cargo.toml")
    }

    fn apply(&self, source_code: &str) -> String {
        delete(&self.target, source_code)
    }
}

/// Rewrite the Cargo.toml source: rename any dependency keyed `from`
/// in the standard dependency tables to `to`.
///
/// Returns the input unchanged when nothing matches or when the file
/// fails to parse.
pub fn rename(from: &str, to: &str, source_code: &str) -> String {
    // Validate as TOML first — refusing to edit malformed files is
    // safer than producing surprising output.
    let Ok(doc) = source_code.parse::<DocumentMut>() else {
        return source_code.to_string();
    };

    if !key_present_in_dep_tables(&doc, from) {
        return source_code.to_string();
    }

    // Walk the source line-by-line, tracking the current section
    // header, and rewrite the leading key in lines that belong to a
    // dependency table.
    let mut out = String::with_capacity(source_code.len());
    let mut current: Option<String> = None;

    for line in source_code.split_inclusive('\n') {
        let trimmed = line.trim();

        // Section header `[name]` (or `[name.sub]`).
        if let Some(name) = section_name(trimmed) {
            // Dotted-table form: `[dependencies.foo]` → rewrite the
            // leaf segment in the header line.
            if let Some(rewritten) = rewrite_dotted_section_header(line, name, from, to) {
                out.push_str(&rewritten);
                // The active section is the rewritten name so any
                // following key lines are tagged correctly. We don't
                // need the section name for any further rename in
                // dotted form (the rename happens on the header
                // itself), so just record the table prefix.
                current = Some(parent_table(name).to_string());
                continue;
            }
            current = Some(parent_table(name).to_string());
            out.push_str(line);
            continue;
        }

        // Inline-key form within a dependency table.
        if current
            .as_deref()
            .map(|s| DEP_TABLES.contains(&s))
            .unwrap_or(false)
            && let Some(rewritten) = rewrite_inline_key_line(line, from, to)
        {
            out.push_str(&rewritten);
            continue;
        }

        out.push_str(line);
    }

    out
}

/// `true` if any of the standard dependency tables contains a key
/// equal to `from` (either inline or via dotted-table form).
fn key_present_in_dep_tables(doc: &DocumentMut, from: &str) -> bool {
    DEP_TABLES.iter().any(|name| {
        doc.get(name)
            .and_then(|item| item.as_table())
            .map(|t| t.contains_key(from))
            .unwrap_or(false)
    })
}

/// Strip surrounding `[` `]`, returning the inner section name. Only
/// returns Some for proper standard table headers (not array-of-table
/// `[[name]]`).
fn section_name(trimmed_line: &str) -> Option<&str> {
    let inner = trimmed_line.strip_prefix('[')?.strip_suffix(']')?;
    if inner.starts_with('[') {
        return None; // [[…]] handled separately if ever needed
    }
    Some(inner.trim())
}

/// Returns the leading segment before the first `.` (e.g.
/// `"dependencies"` for `"dependencies.foo"`).
fn parent_table(section_name: &str) -> &str {
    section_name.split('.').next().unwrap_or(section_name)
}

/// Remove `target` from any of the standard dependency tables.
/// Inline-key entries take their whole line; dotted-table sections
/// take everything from the header through the start of the next
/// section (or end of file).
pub fn delete(target: &str, source_code: &str) -> String {
    let Ok(doc) = source_code.parse::<DocumentMut>() else {
        return source_code.to_string();
    };
    if !key_present_in_dep_tables(&doc, target) {
        return source_code.to_string();
    }

    let mut out = String::with_capacity(source_code.len());
    let mut current: Option<String> = None;
    let mut suppress_section = false;

    for line in source_code.split_inclusive('\n') {
        let trimmed = line.trim();

        // Section header.
        if let Some(name) = section_name(trimmed) {
            // Dotted-table form `[dependencies.<target>]` → drop this
            // section header AND every subsequent line until the next
            // section.
            if let Some((parent, leaf)) = name.rsplit_once('.')
                && DEP_TABLES.contains(&parent)
                && leaf == target
            {
                suppress_section = true;
                current = Some(parent.to_string());
                continue;
            }
            current = Some(parent_table(name).to_string());
            suppress_section = false;
            out.push_str(line);
            continue;
        }

        // Inside a suppressed dotted-table section: drop everything
        // until we hit the next section header (handled above).
        if suppress_section {
            continue;
        }

        // Inline-key form within a dependency table.
        if current
            .as_deref()
            .map(|s| DEP_TABLES.contains(&s))
            .unwrap_or(false)
            && line_declares_key(line, target)
        {
            // Drop the line entirely.
            continue;
        }

        out.push_str(line);
    }

    out
}

/// `true` if `line` declares `target` as a top-level key (whitespace
/// allowed before; `=` or whitespace after).
fn line_declares_key(line: &str, target: &str) -> bool {
    let leading_ws_len = line.len() - line.trim_start().len();
    let after_ws = &line[leading_ws_len..];
    let Some(after_key) = after_ws.strip_prefix(target) else {
        return false;
    };
    after_key.starts_with('=') || after_key.starts_with(' ') || after_key.starts_with('\t')
}

/// If the section header is `[<dep_table>.<from>]`, return a
/// version of `line` with the trailing segment replaced by `to`.
fn rewrite_dotted_section_header(line: &str, name: &str, from: &str, to: &str) -> Option<String> {
    let (parent, leaf) = name.rsplit_once('.')?;
    if !DEP_TABLES.contains(&parent) || leaf != from {
        return None;
    }
    // The line still contains formatting we want to preserve. Replace
    // only the leaf inside the brackets.
    let key_in_line = format!(".{}]", from);
    let replacement = format!(".{}]", to);
    let pos = line.find(&key_in_line)?;
    let mut rewritten = String::with_capacity(line.len() + to.len() - from.len());
    rewritten.push_str(&line[..pos]);
    rewritten.push_str(&replacement);
    rewritten.push_str(&line[pos + key_in_line.len()..]);
    Some(rewritten)
}

/// If `line` declares a key equal to `from` at the start of its
/// non-whitespace content, return a rewritten line with the key
/// replaced. We accept `<key>=` and `<key> =` forms; the suffix can
/// be anything from `"` (string) through `{` (inline table).
fn rewrite_inline_key_line(line: &str, from: &str, to: &str) -> Option<String> {
    let leading_ws_len = line.len() - line.trim_start().len();
    let after_ws = &line[leading_ws_len..];
    let after_key = after_ws.strip_prefix(from)?;
    // Must be followed by a key-value separator; otherwise we'd match
    // any prefix substring (e.g. `from="foo"` but key is "foobar").
    if !(after_key.starts_with('=') || after_key.starts_with(' ') || after_key.starts_with('\t')) {
        return None;
    }
    let key_start = leading_ws_len;
    let key_end = key_start + from.len();
    let mut rewritten = String::with_capacity(line.len() + to.len() - from.len());
    rewritten.push_str(&line[..key_start]);
    rewritten.push_str(to);
    rewritten.push_str(&line[key_end..]);
    Some(rewritten)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_simple_dep() {
        let src = "[dependencies]\nfoo = \"1.0\"\n";
        let out = rename("foo", "bar", src);
        assert_eq!(out, "[dependencies]\nbar = \"1.0\"\n");
    }

    #[test]
    fn renames_inline_table_dep() {
        let src = "[dependencies]\nfoo = { version = \"1.0\", features = [\"x\"] }\n";
        let out = rename("foo", "bar", src);
        assert_eq!(
            out,
            "[dependencies]\nbar = { version = \"1.0\", features = [\"x\"] }\n"
        );
    }

    #[test]
    fn renames_in_dev_dependencies() {
        let src = "[dev-dependencies]\nfoo = \"1.0\"\n";
        let out = rename("foo", "bar", src);
        assert_eq!(out, "[dev-dependencies]\nbar = \"1.0\"\n");
    }

    #[test]
    fn renames_in_build_dependencies() {
        let src = "[build-dependencies]\nfoo = \"1.0\"\n";
        let out = rename("foo", "bar", src);
        assert_eq!(out, "[build-dependencies]\nbar = \"1.0\"\n");
    }

    #[test]
    fn renames_dotted_table_form() {
        let src = "[dependencies.foo]\nversion = \"1.0\"\nfeatures = [\"x\"]\n";
        let out = rename("foo", "bar", src);
        assert_eq!(
            out,
            "[dependencies.bar]\nversion = \"1.0\"\nfeatures = [\"x\"]\n"
        );
    }

    #[test]
    fn preserves_key_order_within_table() {
        let src = "[dependencies]\nalpha = \"1.0\"\nfoo = \"1.0\"\nzeta = \"3.0\"\n";
        let out = rename("foo", "bar", src);
        let alpha_pos = out.find("alpha").unwrap();
        let bar_pos = out.find("bar = ").unwrap();
        let zeta_pos = out.find("zeta").unwrap();
        assert!(alpha_pos < bar_pos && bar_pos < zeta_pos, "got: {out}");
    }

    #[test]
    fn does_not_touch_unrelated_keys() {
        let src = "[dependencies]\nfoo = \"1.0\"\nfoobar = \"2.0\"\n";
        let out = rename("foo", "bar", src);
        // foobar must not be substring-matched.
        assert!(out.contains("foobar = \"2.0\""), "got: {out}");
    }

    #[test]
    fn does_not_touch_other_tables() {
        let src =
            "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n\n[dependencies]\nbaz = \"1.0\"\n";
        let out = rename("foo", "bar", src);
        // `name = "foo"` in [package] must stay.
        assert!(out.contains("name = \"foo\""), "got: {out}");
    }

    #[test]
    fn returns_input_unchanged_when_no_match() {
        let src = "[dependencies]\nbaz = \"1.0\"\n";
        assert_eq!(rename("foo", "bar", src), src);
    }

    #[test]
    fn returns_input_unchanged_on_parse_error() {
        let src = "this is :: not :: valid :: toml";
        assert_eq!(rename("foo", "bar", src), src);
    }

    #[test]
    fn rename_is_idempotent() {
        let src = "[dependencies]\nfoo = \"1.0\"\n";
        let once = rename("foo", "bar", src);
        let twice = rename("foo", "bar", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn preserves_comments_after_rename() {
        let src = "[dependencies]\n# why we depend on foo\nfoo = \"1.0\" # version pinned\n";
        let out = rename("foo", "bar", src);
        assert!(out.contains("# why we depend on foo"));
        assert!(out.contains("# version pinned"));
        assert!(out.contains("bar = \"1.0\""));
    }

    #[test]
    fn deletes_simple_dep() {
        let src = "[dependencies]\nfoo = \"1.0\"\nbar = \"2.0\"\n";
        let out = delete("foo", src);
        assert_eq!(out, "[dependencies]\nbar = \"2.0\"\n");
    }

    #[test]
    fn deletes_inline_table_dep() {
        let src = "[dependencies]\nfoo = { version = \"1.0\" }\nbar = \"2.0\"\n";
        let out = delete("foo", src);
        assert_eq!(out, "[dependencies]\nbar = \"2.0\"\n");
    }

    #[test]
    fn deletes_dotted_table_form() {
        let src = "[dependencies.foo]\nversion = \"1.0\"\n\n[dependencies]\nbar = \"2.0\"\n";
        let out = delete("foo", src);
        assert!(!out.contains("[dependencies.foo]"));
        assert!(!out.contains("version = \"1.0\""));
        assert!(out.contains("bar = \"2.0\""));
    }

    #[test]
    fn delete_in_dev_dependencies_only() {
        let src = "[dependencies]\nfoo = \"1.0\"\n\n[dev-dependencies]\nfoo = \"1.0\"\n";
        let out = delete("foo", src);
        // Both occurrences (in dependencies AND dev-dependencies) are removed.
        assert!(!out.contains("foo = \"1.0\""));
    }

    #[test]
    fn delete_does_not_match_substrings() {
        let src = "[dependencies]\nfoobar = \"2.0\"\n";
        assert_eq!(delete("foo", src), src);
    }

    #[test]
    fn delete_returns_input_unchanged_when_no_match() {
        let src = "[dependencies]\nbar = \"2.0\"\n";
        assert_eq!(delete("foo", src), src);
    }

    #[test]
    fn delete_is_idempotent() {
        let src = "[dependencies]\nfoo = \"1.0\"\n";
        let once = delete("foo", src);
        let twice = delete("foo", &once);
        assert_eq!(once, twice);
    }

    #[test]
    fn crate_rename_operation_is_relevant_for_cargo_toml_only() {
        let op = CrateRename {
            from: "a".into(),
            to: "b".into(),
        };
        assert!(op.is_file_relevant(Path::new("Cargo.toml")));
        assert!(op.is_file_relevant(Path::new("crates/foo/Cargo.toml")));
        assert!(!op.is_file_relevant(Path::new("foo.toml")));
        assert!(!op.is_file_relevant(Path::new("foo.rs")));
    }
}
