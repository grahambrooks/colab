//! A minimal, deterministic language server for colab-lsp-client's tests.
//!
//! It renames every whole-word occurrence of the identifier under the
//! cursor across all files with the same extension in the root. To
//! exercise the client, it also:
//!
//! - asks the client for `workspace/configuration` during start-up,
//! - reports a `$/progress` "indexing" run that ends ~300 ms later,
//! - fails the *first* `textDocument/rename` with `ContentModified`.
//!
//! `FAKE_LSP_MODE` changes behaviour: `no-rename` (no renameProvider),
//! `file-op` (answers with a file rename operation), `slow` (indexing
//! never finishes).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

fn read_frame(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(n) = line.strip_prefix("Content-Length: ") {
            length = n.parse().ok()?;
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn send(out: &mut impl Write, message: Value) {
    let body = message.to_string();
    write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    out.flush().unwrap();
}

fn uri_path(uri: &str) -> PathBuf {
    PathBuf::from(uri.trim_start_matches("file://").replace("%20", " "))
}

fn offset(text: &str, line: u64, character: u64) -> usize {
    let start: usize = text
        .split_inclusive('\n')
        .take(line as usize)
        .map(str::len)
        .sum();
    let line_text = &text[start..];
    let mut units = 0;
    for (i, c) in line_text.char_indices() {
        if units >= character as usize || c == '\n' {
            return start + i;
        }
        units += c.len_utf16();
    }
    text.len()
}

fn position(text: &str, at: usize) -> Value {
    let before = &text[..at];
    let line = before.matches('\n').count();
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    json!({"line": line, "character": before[start..].encode_utf16().count()})
}

fn ident_at(text: &str, at: usize) -> Option<(usize, usize)> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| !is_ident(*c))
        .map_or(text.len(), |(i, _)| at + i);
    (start < end).then_some((start, end))
}

fn occurrences(text: &str, name: &str) -> Vec<usize> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name)
        .map(|(i, _)| i)
        .filter(|&i| {
            text[..i].chars().next_back().is_none_or(|c| !is_ident(c))
                && text[i + name.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !is_ident(c))
        })
        .collect()
}

fn main() {
    let mode = std::env::var("FAKE_LSP_MODE").unwrap_or_default();
    let stdin = std::io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let mut out = std::io::stdout().lock();
    let mut root = PathBuf::from(".");
    let mut open: HashMap<String, String> = HashMap::new();
    let mut renames = 0;

    while let Some(message) = read_frame(&mut input) {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match (method.as_str(), id) {
            ("initialize", Some(id)) => {
                if let Some(uri) = params.get("rootUri").and_then(Value::as_str) {
                    root = uri_path(uri);
                }
                let rename = if mode == "no-rename" {
                    Value::Null
                } else {
                    json!({"prepareProvider": true})
                };
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": {"capabilities": {"renameProvider": rename, "positionEncoding": "utf-16"}}}),
                );
            }
            ("initialized", None) => {
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": "cfg", "method": "workspace/configuration", "params": {"items": [{"section": "fake"}]}}),
                );
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": "wdp", "method": "window/workDoneProgress/create", "params": {"token": "index"}}),
                );
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "index", "value": {"kind": "begin", "title": "Indexing"}}}),
                );
                if mode != "slow" {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    send(
                        &mut out,
                        json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "index", "value": {"kind": "end"}}}),
                    );
                }
            }
            ("textDocument/didOpen", None) => {
                let doc = &params["textDocument"];
                open.insert(
                    doc["uri"].as_str().unwrap().into(),
                    doc["text"].as_str().unwrap().into(),
                );
            }
            ("textDocument/prepareRename", Some(id)) => {
                let uri = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .unwrap();
                let text = &open[uri];
                let at = offset(
                    text,
                    params["position"]["line"].as_u64().unwrap(),
                    params["position"]["character"].as_u64().unwrap(),
                );
                let result = match ident_at(text, at) {
                    Some((s, e)) => {
                        json!({"range": {"start": position(text, s), "end": position(text, e)}, "placeholder": &text[s..e]})
                    }
                    None => Value::Null,
                };
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": result}),
                );
            }
            ("textDocument/rename", Some(id)) => {
                renames += 1;
                if renames == 1 {
                    send(
                        &mut out,
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32801, "message": "content modified"}}),
                    );
                    continue;
                }
                if mode == "file-op" {
                    send(
                        &mut out,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"documentChanges": [{"kind": "rename", "oldUri": "file:///a", "newUri": "file:///b"}]}}),
                    );
                    continue;
                }
                let uri = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .unwrap();
                let text = &open[uri];
                let at = offset(
                    text,
                    params["position"]["line"].as_u64().unwrap(),
                    params["position"]["character"].as_u64().unwrap(),
                );
                let (s, e) = ident_at(text, at).unwrap();
                let name = &text[s..e];
                let new_name = params["newName"].as_str().unwrap();
                let ext = Path::new(&uri_path(uri)).extension().map(|e| e.to_owned());
                let mut changes = serde_json::Map::new();
                let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
                    .unwrap()
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().map(|e| e.to_owned()) == ext)
                    .collect();
                files.sort();
                for file in files {
                    let content = std::fs::read_to_string(&file).unwrap();
                    let edits: Vec<Value> = occurrences(&content, name)
                        .into_iter()
                        .map(|i| json!({"range": {"start": position(&content, i), "end": position(&content, i + name.len())}, "newText": new_name}))
                        .collect();
                    if !edits.is_empty() {
                        changes.insert(format!("file://{}", file.display()), Value::Array(edits));
                    }
                }
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": {"changes": changes}}),
                );
            }
            ("shutdown", Some(id)) => send(
                &mut out,
                json!({"jsonrpc": "2.0", "id": id, "result": null}),
            ),
            ("exit", None) => return,
            (_, Some(id)) if message.get("result").is_some() || message.get("error").is_some() => {
                let _ = id; // a reply to one of our requests
            }
            (_, Some(id)) => send(
                &mut out,
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "unsupported"}}),
            ),
            _ => {}
        }
    }
}
