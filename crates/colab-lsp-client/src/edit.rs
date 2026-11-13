//! Turn an LSP `WorkspaceEdit` into before/after file pairs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use colab_core::fileops::{ChangeSet, FileOp};
use colab_core::walker::FileChange;
use colab_core::{Error, Result};
use colab_rewrite::Edit;
use serde_json::Value;

/// How the server counts columns, as negotiated in `initialize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
    Utf32,
}

impl Encoding {
    pub fn parse(name: Option<&str>) -> Self {
        match name {
            Some("utf-8") => Self::Utf8,
            Some("utf-32") => Self::Utf32,
            _ => Self::Utf16,
        }
    }

    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf16 => "utf-16",
            Self::Utf32 => "utf-32",
        }
    }

    fn units(self, c: char) -> u32 {
        match self {
            Self::Utf8 => c.len_utf8() as u32,
            Self::Utf16 => c.len_utf16() as u32,
            Self::Utf32 => 1,
        }
    }
}

/// Byte offset of a 0-based LSP `line`/`character` in `text`.
pub fn offset_of(text: &str, line: u32, character: u32, encoding: Encoding) -> Result<usize> {
    let mut start = 0usize;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => {
                return Err(Error::Config(format!(
                    "language server sent line {} past the end of a {}-line file",
                    line + 1,
                    text.lines().count()
                )));
            }
        }
    }
    let end = text[start..]
        .find('\n')
        .map(|i| start + i)
        .unwrap_or(text.len());
    let mut units = 0u32;
    for (i, c) in text[start..end].char_indices() {
        if units >= character {
            return Ok(start + i);
        }
        units += encoding.units(c);
    }
    Ok(end)
}

/// 0-based LSP `(line, character)` of byte `offset`.
pub fn position_of(text: &str, offset: usize, encoding: Encoding) -> (u32, u32) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() as u32;
    let start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = before[start..].chars().map(|c| encoding.units(c)).sum();
    (line, character)
}

/// `file://` URI for `path` (which must be absolute).
pub fn path_to_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !text.starts_with('/') {
        out.push('/'); // Windows drive path: file:///C:/…
    }
    for b in text.bytes() {
        let keep = b.is_ascii_alphanumeric() || b"/-_.~:".contains(&b);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The path a `file://` URI names.
pub fn uri_to_path(uri: &str) -> Result<PathBuf> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| Error::Config(format!("language server sent a non-file URI: {uri}")))?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&rest[i + 1..i + 3], 16)
        {
            decoded.push(byte);
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    let text = String::from_utf8(decoded)
        .map_err(|_| Error::Config(format!("language server sent a non-UTF-8 URI: {uri}")))?;
    // `/C:/x` → `C:/x` on Windows.
    let text = match text.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => text[1..].to_string(),
        _ => text,
    };
    Ok(PathBuf::from(text))
}

/// What [`to_change_set`] needs to know about the files on disk.
pub trait Files {
    fn read(&self, path: &Path) -> Result<String>;
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
}

/// The real filesystem.
pub struct Disk;

impl Files for Disk {
    fn read(&self, path: &Path) -> Result<String> {
        std::fs::read_to_string(path).map_err(|e| Error::io_at(path, e))
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
}

/// Every text edit in a `WorkspaceEdit`, refusing file operations — for
/// callers that cannot make them (outside a git work tree).
pub fn to_changes<R>(edit: &Value, encoding: Encoding, read: R) -> Result<Vec<FileChange>>
where
    R: Fn(&Path) -> Result<String>,
{
    struct Reader<R>(R);
    impl<R: Fn(&Path) -> Result<String>> Files for Reader<R> {
        fn read(&self, path: &Path) -> Result<String> {
            (self.0)(path)
        }
        fn exists(&self, _: &Path) -> bool {
            true
        }
        fn is_dir(&self, _: &Path) -> bool {
            false
        }
    }
    Ok(to_change_set(edit, encoding, &Reader(read), false)?.edits)
}

/// The end state a `WorkspaceEdit` describes: its file operations in
/// order, then one before/after pair per file that ends up with different
/// content, sorted by path.
///
/// Both shapes are accepted: `changes` (URI → edits) and
/// `documentChanges`, whose entries are applied in order. With
/// `file_operations` false, a create / rename / delete is refused rather
/// than half-applied; with it true they are replayed on a virtual copy of
/// the files, so an edit after a rename lands on the renamed file and an
/// edit's `before` is always the file's original content.
pub fn to_change_set(
    edit: &Value,
    encoding: Encoding,
    files: &dyn Files,
    file_operations: bool,
) -> Result<ChangeSet> {
    let mut vfs = Vfs {
        files,
        entries: BTreeMap::new(),
        ops: Vec::new(),
    };
    if let Some(document_changes) = edit.get("documentChanges").and_then(Value::as_array) {
        for change in document_changes {
            if let Some(kind) = change.get("kind").and_then(Value::as_str) {
                if !file_operations {
                    return Err(Error::UnsupportedOperation(format!(
                        "the language server's edit would {kind} a file; colab makes file operations only inside a git work tree (with git mv / add / rm, so history and undo survive)"
                    )));
                }
                vfs.resource_operation(kind, change)?;
                continue;
            }
            let uri = change
                .pointer("/textDocument/uri")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Config("documentChanges entry without a URI".into()))?;
            let edits: Vec<&Value> = change
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .collect();
            vfs.text_edits(&uri_to_path(uri)?, &edits, encoding)?;
        }
    } else if let Some(changes) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            let edits: Vec<&Value> = edits.as_array().into_iter().flatten().collect();
            vfs.text_edits(&uri_to_path(uri)?, &edits, encoding)?;
        }
    }

    let edits = vfs
        .entries
        .into_iter()
        .filter(|(_, e)| e.before != e.after)
        .map(|(path, e)| FileChange {
            path,
            before: e.before,
            after: e.after,
            rules_fired: vec![0],
        })
        .collect();
    Ok(ChangeSet {
        ops: vfs.ops,
        edits,
    })
}

/// A file as the edit so far has left it.
struct Entry {
    /// Its content before the edit (`""` for a file the edit created).
    before: String,
    after: String,
}

/// The files a `WorkspaceEdit` has touched so far, keyed by where they are
/// now, over the real files it has not.
struct Vfs<'a> {
    files: &'a dyn Files,
    entries: BTreeMap<PathBuf, Entry>,
    ops: Vec<FileOp>,
}

impl Vfs<'_> {
    /// Where the file now at `path` was on disk, undoing the moves made so
    /// far; `None` if the edit created it.
    fn origin(&self, path: &Path) -> Option<PathBuf> {
        ChangeSet {
            ops: self.ops.clone(),
            edits: Vec::new(),
        }
        .origin_of(path)
    }

    fn exists(&self, path: &Path) -> bool {
        if self.entries.contains_key(path) {
            return true;
        }
        if self
            .ops
            .iter()
            .any(|op| matches!(op, FileOp::Delete { path: p, .. } if p == path))
        {
            return false;
        }
        // Something moved away from here is gone; something moved here is not
        // on disk yet, but is an entry or a directory move's target.
        if self
            .ops
            .iter()
            .any(|op| matches!(op, FileOp::Move { from, .. } if path.starts_with(from)))
        {
            return false;
        }
        match self.origin(path) {
            Some(origin) => self.files.exists(&origin),
            None => false,
        }
    }

    fn load(&mut self, path: &Path) -> Result<&mut Entry> {
        if !self.entries.contains_key(path) {
            let origin = self.origin(path).filter(|o| self.files.exists(o));
            let before = match origin {
                Some(origin) => self.files.read(&origin)?,
                None => {
                    return Err(Error::Config(format!(
                        "language server edited {}, which does not exist",
                        path.display()
                    )));
                }
            };
            self.entries.insert(
                path.to_path_buf(),
                Entry {
                    after: before.clone(),
                    before,
                },
            );
        }
        Ok(self.entries.get_mut(path).expect("inserted above"))
    }

    fn text_edits(&mut self, path: &Path, edits: &[&Value], encoding: Encoding) -> Result<()> {
        let entry = self.load(path)?;
        let text = &entry.after;
        let mut pending = Vec::with_capacity(edits.len());
        for edit in edits {
            let at = |field: &str| -> Result<usize> {
                let line = edit
                    .pointer(&format!("/range/{field}/line"))
                    .and_then(Value::as_u64);
                let character = edit
                    .pointer(&format!("/range/{field}/character"))
                    .and_then(Value::as_u64);
                match (line, character) {
                    (Some(l), Some(c)) => offset_of(text, l as u32, c as u32, encoding),
                    _ => Err(Error::Config("text edit without a range".into())),
                }
            };
            let new_text = edit.get("newText").and_then(Value::as_str).unwrap_or("");
            pending.push(Edit::new(at("start")?, at("end")?, new_text));
        }
        entry.after = colab_rewrite::apply_edits(text, pending);
        Ok(())
    }

    fn resource_operation(&mut self, kind: &str, change: &Value) -> Result<()> {
        let option = |name: &str| {
            change
                .pointer(&format!("/options/{name}"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        let uri = |field: &str| -> Result<PathBuf> {
            let uri = change.get(field).and_then(Value::as_str).ok_or_else(|| {
                Error::Config(format!("`{kind}` file operation without `{field}`"))
            })?;
            Ok(PathBuf::from(
                uri_to_path(uri)?.to_string_lossy().trim_end_matches('/'),
            ))
        };
        match kind {
            "create" => {
                let path = uri("uri")?;
                if self.exists(&path) {
                    if option("ignoreIfExists") && !option("overwrite") {
                        return Ok(());
                    }
                    return Err(Error::UnsupportedOperation(format!(
                        "the language server would overwrite {} by creating it; colab does not overwrite files",
                        path.display()
                    )));
                }
                self.entries.insert(
                    path.clone(),
                    Entry {
                        before: String::new(),
                        after: String::new(),
                    },
                );
                self.ops.push(FileOp::Create { path });
            }
            "rename" => {
                let (from, to) = (uri("oldUri")?, uri("newUri")?);
                if from == to {
                    return Ok(());
                }
                if self.exists(&to) {
                    if option("ignoreIfExists") && !option("overwrite") {
                        return Ok(());
                    }
                    return Err(Error::UnsupportedOperation(format!(
                        "the language server would move {} onto existing {}; colab does not overwrite files",
                        from.display(),
                        to.display()
                    )));
                }
                let is_dir = self.origin(&from).is_some_and(|o| self.files.is_dir(&o));
                if !is_dir && !self.exists(&from) {
                    return Err(Error::Config(format!(
                        "the language server would move {}, which does not exist",
                        from.display()
                    )));
                }
                // Re-key everything already loaded under the old path.
                let moved: Vec<PathBuf> = self
                    .entries
                    .keys()
                    .filter(|p| p.starts_with(&from))
                    .cloned()
                    .collect();
                for old in moved {
                    let entry = self.entries.remove(&old).expect("listed above");
                    let rest = old.strip_prefix(&from).expect("filtered above");
                    let new = if rest.as_os_str().is_empty() {
                        to.clone()
                    } else {
                        to.join(rest)
                    };
                    self.entries.insert(new, entry);
                }
                self.ops.push(FileOp::Move { from, to });
            }
            "delete" => {
                let path = uri("uri")?;
                let is_dir = self.origin(&path).is_some_and(|o| self.files.is_dir(&o));
                if is_dir {
                    return Err(Error::UnsupportedOperation(format!(
                        "the language server would delete the directory {}; colab deletes files only",
                        path.display()
                    )));
                }
                if !self.exists(&path) {
                    if option("ignoreIfNotExists") {
                        return Ok(());
                    }
                    return Err(Error::Config(format!(
                        "the language server would delete {}, which does not exist",
                        path.display()
                    )));
                }
                let created = self.origin(&path).is_none();
                let before = match self.entries.remove(&path) {
                    Some(entry) => entry.before,
                    None => self.files.read(&self.origin(&path).expect("exists"))?,
                };
                if created {
                    // Created and deleted within one edit: nothing happens.
                    self.ops
                        .retain(|op| !matches!(op, FileOp::Create { path: p } if *p == path));
                } else {
                    self.ops.push(FileOp::Delete { path, before });
                }
            }
            other => {
                return Err(Error::Config(format!(
                    "unknown file operation `{other}` in the language server's edit"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn utf16_and_utf8_columns_resolve_to_the_same_byte() {
        let text = "a\nµ€x\n";
        let x = text.find('x').unwrap();
        assert_eq!(offset_of(text, 1, 2, Encoding::Utf16).unwrap(), x);
        assert_eq!(offset_of(text, 1, 5, Encoding::Utf8).unwrap(), x);
        assert_eq!(position_of(text, x, Encoding::Utf16), (1, 2));
        assert_eq!(position_of(text, x, Encoding::Utf8), (1, 5));
    }

    #[test]
    fn uris_round_trip_with_escapes() {
        let path = Path::new("/tmp/my dir/ü.rs");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///tmp/my%20dir/%C3%BC.rs");
        assert_eq!(uri_to_path(&uri).unwrap(), path);
    }

    #[test]
    fn both_edit_shapes_apply_and_merge_per_file() {
        let a = "fn old() {}\nfn main() { old(); }\n";
        let read = |p: &Path| -> Result<String> {
            assert_eq!(p, Path::new("/w/a.rs"));
            Ok(a.to_string())
        };
        let edit = |l: u32, s: u32, e: u32| json!({"range": {"start": {"line": l, "character": s}, "end": {"line": l, "character": e}}, "newText": "new"});
        let changes = json!({"changes": {"file:///w/a.rs": [edit(0, 3, 6), edit(1, 12, 15)]}});
        let out = to_changes(&changes, Encoding::Utf16, read).unwrap();
        assert_eq!(out[0].after, "fn new() {}\nfn main() { new(); }\n");

        let documents = json!({"documentChanges": [
            {"textDocument": {"uri": "file:///w/a.rs", "version": 1}, "edits": [edit(0, 3, 6)]},
            {"textDocument": {"uri": "file:///w/a.rs", "version": 1}, "edits": [edit(1, 12, 15)]}
        ]});
        let out = to_changes(&documents, Encoding::Utf16, read).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].after, "fn new() {}\nfn main() { new(); }\n");
    }

    #[test]
    fn file_operations_are_refused_outside_git() {
        let edit = json!({"documentChanges": [{"kind": "rename", "oldUri": "file:///a", "newUri": "file:///b"}]});
        let err = to_changes(&edit, Encoding::Utf16, |_| Ok(String::new())).unwrap_err();
        assert!(err.to_string().contains("would rename a file"), "{err}");
        assert!(err.to_string().contains("git work tree"), "{err}");
    }

    /// An in-memory tree for the virtual-filesystem tests.
    struct Fake(BTreeMap<PathBuf, String>);

    impl Files for Fake {
        fn read(&self, path: &Path) -> Result<String> {
            self.0
                .get(path)
                .cloned()
                .ok_or_else(|| Error::Config(format!("no {}", path.display())))
        }
        fn exists(&self, path: &Path) -> bool {
            self.0.contains_key(path) || self.is_dir(path)
        }
        fn is_dir(&self, path: &Path) -> bool {
            self.0.keys().any(|p| p != path && p.starts_with(path))
        }
    }

    fn fake(files: &[(&str, &str)]) -> Fake {
        Fake(
            files
                .iter()
                .map(|(p, c)| (PathBuf::from(p), c.to_string()))
                .collect(),
        )
    }

    fn text_edit(uri: &str, line: u32, start: u32, end: u32, new: &str) -> Value {
        json!({"textDocument": {"uri": uri, "version": null}, "edits": [{"range": {"start": {"line": line, "character": start}, "end": {"line": line, "character": end}}, "newText": new}]})
    }

    #[test]
    fn a_rename_then_an_edit_lands_on_the_moved_file_with_its_original_before() {
        let files = fake(&[
            ("/w/Old.java", "class Old {}\n"),
            ("/w/Use.java", "Old o;\n"),
        ]);
        let edit = json!({"documentChanges": [
            text_edit("file:///w/Use.java", 0, 0, 3, "New"),
            {"kind": "rename", "oldUri": "file:///w/Old.java", "newUri": "file:///w/New.java"},
            text_edit("file:///w/New.java", 0, 6, 9, "New"),
        ]});
        let set = to_change_set(&edit, Encoding::Utf16, &files, true).unwrap();
        assert_eq!(
            set.ops,
            vec![FileOp::Move {
                from: "/w/Old.java".into(),
                to: "/w/New.java".into()
            }]
        );
        assert_eq!(set.edits.len(), 2);
        assert_eq!(set.edits[0].path, Path::new("/w/New.java"));
        assert_eq!(set.edits[0].before, "class Old {}\n");
        assert_eq!(set.edits[0].after, "class New {}\n");
        assert_eq!(set.edits[1].after, "New o;\n");
    }

    #[test]
    fn a_pure_move_has_no_edit() {
        let files = fake(&[("/w/a.rs", "x\n")]);
        let edit = json!({"documentChanges": [{"kind": "rename", "oldUri": "file:///w/a.rs", "newUri": "file:///w/b.rs"}]});
        let set = to_change_set(&edit, Encoding::Utf16, &files, true).unwrap();
        assert_eq!(set.ops.len(), 1);
        assert!(set.edits.is_empty());
    }

    #[test]
    fn edits_under_a_moved_directory_read_the_original_files() {
        let files = fake(&[("/w/src/m.rs", "fn a() {}\n")]);
        let edit = json!({"documentChanges": [
            {"kind": "rename", "oldUri": "file:///w/src", "newUri": "file:///w/lib"},
            text_edit("file:///w/lib/m.rs", 0, 3, 4, "b"),
        ]});
        let set = to_change_set(&edit, Encoding::Utf16, &files, true).unwrap();
        assert_eq!(set.edits[0].path, Path::new("/w/lib/m.rs"));
        assert_eq!(set.edits[0].before, "fn a() {}\n");
        assert_eq!(set.edits[0].after, "fn b() {}\n");
    }

    #[test]
    fn creates_and_deletes_are_recorded_and_cancel_out() {
        let files = fake(&[("/w/gone.rs", "old\n")]);
        let edit = json!({"documentChanges": [
            {"kind": "create", "uri": "file:///w/new.rs"},
            text_edit("file:///w/new.rs", 0, 0, 0, "fresh\n"),
            {"kind": "delete", "uri": "file:///w/gone.rs"},
            {"kind": "create", "uri": "file:///w/tmp.rs"},
            {"kind": "delete", "uri": "file:///w/tmp.rs"},
        ]});
        let set = to_change_set(&edit, Encoding::Utf16, &files, true).unwrap();
        assert_eq!(
            set.ops,
            vec![
                FileOp::Create {
                    path: "/w/new.rs".into()
                },
                FileOp::Delete {
                    path: "/w/gone.rs".into(),
                    before: "old\n".into()
                },
            ]
        );
        assert_eq!(set.edits[0].before, "");
        assert_eq!(set.edits[0].after, "fresh\n");
    }

    #[test]
    fn overwriting_and_missing_targets_are_refused() {
        let files = fake(&[("/w/a.rs", ""), ("/w/b.rs", "")]);
        let onto = json!({"documentChanges": [{"kind": "rename", "oldUri": "file:///w/a.rs", "newUri": "file:///w/b.rs", "options": {"overwrite": true}}]});
        let err = to_change_set(&onto, Encoding::Utf16, &files, true).unwrap_err();
        assert!(err.to_string().contains("does not overwrite"), "{err}");
        let skipped = json!({"documentChanges": [{"kind": "create", "uri": "file:///w/a.rs", "options": {"ignoreIfExists": true}}]});
        assert!(
            to_change_set(&skipped, Encoding::Utf16, &files, true)
                .unwrap()
                .ops
                .is_empty()
        );
        let missing = json!({"documentChanges": [{"kind": "delete", "uri": "file:///w/zz.rs"}]});
        assert!(to_change_set(&missing, Encoding::Utf16, &files, true).is_err());
    }
}
