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
//! It also answers `workspace/willRenameFiles` by renaming whole-word
//! occurrences of the moved file's stem (think `mod old;`) in every file
//! with the same extension.
//!
//! For M25 it also offers code actions (an "extract into variable" that
//! needs `codeAction/resolve`, an "uppercase file" that is a command whose
//! edit arrives as `workspace/applyEdit`, a disabled one, and "organize
//! imports", which sorts the leading `use` lines), `references`
//! (whole-word occurrences; the `fn NAME` one is the declaration),
//! `documentSymbol` (one per `fn` item), and `formatting` (strips
//! trailing whitespace).
//!
//! For M27 it publishes diagnostics on every open and change: an error per
//! line containing `BROKEN`, a warning per line containing `WARN`.
//!
//! `FAKE_LSP_MODE` changes behaviour: `no-rename` (no renameProvider),
//! `file-op` (a rename also renames the symbol's file to `<new>.<ext>`,
//! like a Java class), `no-will-rename` (no willRenameFiles support),
//! `slow` (indexing never finishes).

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

/// Edits renaming whole-word `name` to `new_name` in every file of the
/// root with extension `ext`, keyed by URI.
fn rename_everywhere(
    root: &Path,
    ext: Option<&std::ffi::OsStr>,
    name: &str,
    new_name: &str,
) -> serde_json::Map<String, Value> {
    let mut changes = serde_json::Map::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension() == ext)
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
    changes
}

/// `publishDiagnostics` for `text`: one error per line containing
/// `BROKEN`, and a warning (which `--verify-lsp` ignores) per `WARN`.
fn publish(uri: &str, text: &str) -> Value {
    let mut diagnostics = Vec::new();
    for (line, content) in text.lines().enumerate() {
        for (word, severity) in [("BROKEN", 1), ("WARN", 2)] {
            if content.contains(word) {
                diagnostics.push(json!({
                    "range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": 1}},
                    "severity": severity,
                    "message": format!("{} here: {}", word.to_lowercase(), content.trim()),
                }));
            }
        }
    }
    json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": diagnostics}})
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
                let mut capabilities = json!({
                    "renameProvider": rename,
                    "positionEncoding": "utf-16",
                    "codeActionProvider": {"resolveProvider": true},
                    "referencesProvider": true,
                    "documentSymbolProvider": true,
                    "documentFormattingProvider": true,
                    "executeCommandProvider": {"commands": ["fake.uppercase"]},
                });
                if mode != "no-will-rename" {
                    capabilities["workspace"] = json!({"fileOperations": {"willRename": {"filters": [{"pattern": {"glob": "**/*"}}]}}});
                }
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": {"capabilities": capabilities}}),
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
                let uri = doc["uri"].as_str().unwrap().to_string();
                let text = doc["text"].as_str().unwrap().to_string();
                send(&mut out, publish(&uri, &text));
                open.insert(uri, text);
            }
            ("textDocument/didChange", None) => {
                let uri = params["textDocument"]["uri"].as_str().unwrap().to_string();
                let text = params["contentChanges"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_string();
                send(&mut out, publish(&uri, &text));
                open.insert(uri, text);
            }
            ("textDocument/codeAction", Some(id)) => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let text = &open[uri];
                let start = offset(
                    text,
                    params["range"]["start"]["line"].as_u64().unwrap(),
                    params["range"]["start"]["character"].as_u64().unwrap(),
                );
                let end = offset(
                    text,
                    params["range"]["end"]["line"].as_u64().unwrap(),
                    params["range"]["end"]["character"].as_u64().unwrap(),
                );
                let mut actions = vec![
                    json!({"title": "Uppercase file", "kind": "refactor.rewrite.uppercase",
                           "command": {"title": "Uppercase", "command": "fake.uppercase", "arguments": [uri]}}),
                    json!({"title": "Convert to async", "kind": "refactor.rewrite.async",
                           "disabled": {"reason": "not a function"}}),
                    json!({"title": "Organize imports", "kind": "source.organizeImports",
                           "data": {"uri": uri, "op": "organize"}}),
                ];
                if start < end {
                    actions.insert(0, json!({"title": "Extract into variable", "kind": "refactor.extract.variable",
                           "data": {"uri": uri, "op": "extract", "start": start, "end": end}}));
                }
                let only: Vec<String> = params
                    .pointer("/context/only")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|k| k.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let actions: Vec<Value> = actions
                    .into_iter()
                    .filter(|a| {
                        let kind = a["kind"].as_str().unwrap_or("");
                        only.is_empty()
                            || only
                                .iter()
                                .any(|k| kind == k || kind.starts_with(&format!("{k}.")))
                    })
                    .collect();
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": actions}),
                );
            }
            ("codeAction/resolve", Some(id)) => {
                let mut action = params.clone();
                let data = &params["data"];
                if data.is_null() {
                    send(
                        &mut out,
                        json!({"jsonrpc": "2.0", "id": id, "result": action}),
                    );
                    continue;
                }
                let uri = data["uri"].as_str().unwrap();
                let text = open[uri].clone();
                let edits = match data["op"].as_str().unwrap() {
                    "extract" => {
                        let (s, e) = (
                            data["start"].as_u64().unwrap() as usize,
                            data["end"].as_u64().unwrap() as usize,
                        );
                        let line_start = text[..s].rfind('\n').map_or(0, |i| i + 1);
                        let indent: String = text[line_start..]
                            .chars()
                            .take_while(|c| *c == ' ')
                            .collect();
                        vec![
                            json!({"range": {"start": position(&text, line_start), "end": position(&text, line_start)},
                                   "newText": format!("{indent}let extracted = {};\n", &text[s..e])}),
                            json!({"range": {"start": position(&text, s), "end": position(&text, e)}, "newText": "extracted"}),
                        ]
                    }
                    _ => {
                        // Sort the leading `use` lines.
                        let lines: Vec<&str> = text.split_inclusive('\n').collect();
                        let n = lines.iter().take_while(|l| l.starts_with("use ")).count();
                        let mut uses: Vec<&str> = lines[..n].to_vec();
                        uses.sort();
                        let end: usize = lines[..n].iter().map(|l| l.len()).sum();
                        vec![
                            json!({"range": {"start": position(&text, 0), "end": position(&text, end)}, "newText": uses.concat()}),
                        ]
                    }
                };
                action["edit"] = json!({"changes": {uri: edits}});
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": action}),
                );
            }
            ("workspace/executeCommand", Some(id)) => {
                let uri = params["arguments"][0].as_str().unwrap();
                let text = open[uri].clone();
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": "apply-1", "method": "workspace/applyEdit", "params": {"edit": {"changes": {uri: [
                        {"range": {"start": position(&text, 0), "end": position(&text, text.len())}, "newText": text.to_uppercase()}
                    ]}}}}),
                );
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": null}),
                );
            }
            ("textDocument/references", Some(id)) => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let text = &open[uri];
                let at = offset(
                    text,
                    params["position"]["line"].as_u64().unwrap(),
                    params["position"]["character"].as_u64().unwrap(),
                );
                let (s, e) = ident_at(text, at).unwrap();
                let name = text[s..e].to_string();
                let include = params["context"]["includeDeclaration"]
                    .as_bool()
                    .unwrap_or(true);
                let ext = uri_path(uri).extension().map(|e| e.to_owned());
                let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
                    .unwrap()
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().map(|e| e.to_owned()) == ext)
                    .collect();
                files.sort();
                let mut locations = Vec::new();
                for file in files {
                    let content = std::fs::read_to_string(&file).unwrap();
                    for i in occurrences(&content, &name) {
                        let declaration = content[..i].ends_with("fn ");
                        if declaration && !include {
                            continue;
                        }
                        locations.push(json!({"uri": format!("file://{}", file.display()),
                            "range": {"start": position(&content, i), "end": position(&content, i + name.len())}}));
                    }
                }
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": locations}),
                );
            }
            ("textDocument/documentSymbol", Some(id)) => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let text = &open[uri];
                let mut symbols = Vec::new();
                let mut at = 0;
                for line in text.split_inclusive('\n') {
                    if let Some(rest) = line.trim_start().strip_prefix("fn ") {
                        let name: String = rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        let name_at = at + line.find(&format!("fn {name}")).unwrap() + 3;
                        // A one-line item, or up to the next line starting with `}`.
                        let end = if line.contains('}') {
                            at + line.trim_end().len()
                        } else {
                            let tail = &text[at..];
                            at + tail.find("\n}").map_or(tail.len(), |i| i + 2)
                        };
                        symbols.push(json!({"name": name, "kind": 12,
                            "range": {"start": position(text, at), "end": position(text, end)},
                            "selectionRange": {"start": position(text, name_at), "end": position(text, name_at + name.len())}}));
                    }
                    at += line.len();
                }
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": symbols}),
                );
            }
            ("textDocument/formatting", Some(id)) => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let text = &open[uri];
                let formatted: String = text
                    .lines()
                    .map(|l| format!("{}\n", l.trim_end()))
                    .collect();
                let edits = if formatted == *text {
                    json!([])
                } else {
                    json!([{"range": {"start": position(text, 0), "end": position(text, text.len())}, "newText": formatted}])
                };
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": edits}),
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
                let changes = rename_everywhere(&root, ext.as_deref(), name, new_name);
                if mode == "file-op" {
                    // documentChanges: the text edits, then the file itself.
                    let mut document_changes: Vec<Value> = changes
                        .into_iter()
                        .map(|(uri, edits)| json!({"textDocument": {"uri": uri, "version": null}, "edits": edits}))
                        .collect();
                    let file = uri_path(uri);
                    let moved = file.with_file_name(format!(
                        "{}.{}",
                        new_name,
                        file.extension().unwrap().to_string_lossy()
                    ));
                    document_changes.push(json!({"kind": "rename", "oldUri": uri, "newUri": format!("file://{}", moved.display())}));
                    send(
                        &mut out,
                        json!({"jsonrpc": "2.0", "id": id, "result": {"documentChanges": document_changes}}),
                    );
                    continue;
                }
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": {"changes": changes}}),
                );
            }
            ("workspace/willRenameFiles", Some(id)) => {
                let mut changes = serde_json::Map::new();
                for file in params["files"].as_array().unwrap() {
                    let old = uri_path(file["oldUri"].as_str().unwrap());
                    let new = uri_path(file["newUri"].as_str().unwrap());
                    let stem = |p: &Path| p.file_stem().unwrap().to_string_lossy().into_owned();
                    let ext = old.extension().map(|e| e.to_owned());
                    changes.extend(rename_everywhere(
                        &root,
                        ext.as_deref(),
                        &stem(&old),
                        &stem(&new),
                    ));
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
