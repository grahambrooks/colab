//! Turn an LSP `WorkspaceEdit` into before/after file pairs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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

/// Every file a `WorkspaceEdit` changes, as before/after pairs sorted by
/// path. `read` loads a file's current contents.
///
/// Both shapes are accepted: `changes` (URI → edits) and
/// `documentChanges` (text-document edits). Resource operations — create,
/// rename, or delete a *file* — are refused rather than half-applied:
/// colab reports and reverts text, not file moves.
pub fn to_changes<R>(edit: &Value, encoding: Encoding, mut read: R) -> Result<Vec<FileChange>>
where
    R: FnMut(&Path) -> Result<String>,
{
    let mut per_file: BTreeMap<PathBuf, Vec<&Value>> = BTreeMap::new();
    if let Some(document_changes) = edit.get("documentChanges").and_then(Value::as_array) {
        for change in document_changes {
            if let Some(kind) = change.get("kind").and_then(Value::as_str) {
                return Err(Error::UnsupportedOperation(format!(
                    "the language server's edit would {kind} a file; colab applies text edits only — run the rename in an editor instead"
                )));
            }
            let uri = change
                .pointer("/textDocument/uri")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Config("documentChanges entry without a URI".into()))?;
            let edits = change
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            per_file.entry(uri_to_path(uri)?).or_default().extend(edits);
        }
    } else if let Some(changes) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            let edits = edits.as_array().into_iter().flatten();
            per_file.entry(uri_to_path(uri)?).or_default().extend(edits);
        }
    }

    let mut out = Vec::new();
    for (path, edits) in per_file {
        let before = read(&path)?;
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
                    (Some(l), Some(c)) => offset_of(&before, l as u32, c as u32, encoding),
                    _ => Err(Error::Config("text edit without a range".into())),
                }
            };
            let text = edit.get("newText").and_then(Value::as_str).unwrap_or("");
            pending.push(Edit::new(at("start")?, at("end")?, text));
        }
        let after = colab_rewrite::apply_edits(&before, pending);
        out.push(FileChange {
            path,
            before,
            after,
            rules_fired: vec![0],
        });
    }
    Ok(out)
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
    fn file_operations_are_refused() {
        let edit = json!({"documentChanges": [{"kind": "rename", "oldUri": "file:///a", "newUri": "file:///b"}]});
        let err = to_changes(&edit, Encoding::Utf16, |_| Ok(String::new())).unwrap_err();
        assert!(err.to_string().contains("would rename a file"), "{err}");
    }
}
