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
//! For M32 it answers `hover` and `definition` for the identifier at the
//! position: hover shows its declaration line from the open file (`var x
//! T` or `func x(…) T`, as gopls would), and definition finds `func x(`
//! in the directory named by the qualifier before it (`alog.Print` looks
//! in `alog/`), or in the same file without one.
//!
//! It also answers the call and type hierarchies, Go-like: a call
//! hierarchy item is a `func NAME(` declaration, its outgoing calls are
//! the `f(` / `q.f(` calls in its body (resolved like `definition`), and
//! its incoming calls are the functions under the root whose bodies call
//! it. A type's supertypes are named by a comment on its declaration,
//! `type T struct{} // implements: alog.Writer`, each found as `type
//! Writer interface` in `alog/`.
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
        .take(usize::try_from(line).unwrap_or(usize::MAX))
        .map(str::len)
        .sum();
    let line_text = &text[start..];
    let mut units = 0;
    for (i, c) in line_text.char_indices() {
        if units >= usize::try_from(character).unwrap_or(usize::MAX) || c == '\n' {
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

/// The text of `file`: as the client opened it, else from disk.
fn text_of(open: &HashMap<String, String>, file: &Path) -> Option<String> {
    open.get(&format!("file://{}", file.display()))
        .cloned()
        .or_else(|| std::fs::read_to_string(file).ok())
}

/// The `.go` files of the root and its package directories, sorted.
fn go_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "go") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Each `func NAME(` in `text`: its name, where the name starts, and its
/// body (from the `{` to the next top-level `func` or the end).
fn funcs(text: &str) -> Vec<(String, usize, std::ops::Range<usize>)> {
    let starts: Vec<usize> = text.match_indices("func ").map(|(i, _)| i).collect();
    starts
        .iter()
        .enumerate()
        .filter_map(|(k, &i)| {
            let at = i + "func ".len();
            let (s, e) = ident_at(text, at)?;
            let body = text[e..].find('{').map(|b| e + b)?;
            let end = starts.get(k + 1).copied().unwrap_or(text.len());
            Some((text[s..e].to_string(), s, body..end))
        })
        .collect()
}

/// A hierarchy item for the declaration of `name` at `at` in `file`.
fn item(file: &Path, text: &str, name: &str, at: usize, kind: u32) -> Value {
    let range = json!({"start": position(text, at), "end": position(text, at + name.len())});
    json!({"name": name, "kind": kind, "uri": format!("file://{}", file.display()),
        "range": range, "selectionRange": range})
}

/// The declaration a call `qualifier.name(` in `here` refers to: `func
/// name(` in the qualifier's directory, or in `here`'s without one.
fn declaration(
    open: &HashMap<String, String>,
    root: &Path,
    here: &Path,
    qualifier: Option<&str>,
    name: &str,
) -> Option<Value> {
    let dir = match qualifier {
        Some(q) => root.join(q),
        None => here.parent()?.to_path_buf(),
    };
    go_files(root)
        .into_iter()
        .filter(|f| f.parent() == Some(dir.as_path()))
        .find_map(|file| {
            let text = text_of(open, &file)?;
            let (_, at, _) = funcs(&text).into_iter().find(|(n, _, _)| n == name)?;
            Some(item(&file, &text, name, at, 12))
        })
}

/// The calls in `body` of `text`: `(qualifier, name)` for each `f(` or
/// `q.f(`.
fn calls_in(text: &str, body: std::ops::Range<usize>) -> Vec<(Option<String>, String)> {
    text[body.clone()]
        .match_indices('(')
        .filter_map(|(i, _)| {
            let at = body.start + i;
            let (s, e) = ident_at(text, at.checked_sub(1)?)?;
            (e == at).then_some(())?;
            let qualifier = text[..s]
                .strip_suffix('.')
                .and_then(|before| ident_at(before, before.len()))
                .map(|(qs, qe)| text[qs..qe].to_string());
            Some((qualifier, text[s..e].to_string()))
        })
        .collect()
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
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "callHierarchyProvider": true,
                    "typeHierarchyProvider": true,
                    "implementationProvider": true,
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
                    .map_or_default(|a| {
                        a.iter()
                            .filter_map(|k| k.as_str().map(str::to_string))
                            .collect()
                    });
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
                            usize::try_from(data["start"].as_u64().unwrap()).unwrap(),
                            usize::try_from(data["end"].as_u64().unwrap()).unwrap(),
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
            ("textDocument/hover", Some(id)) => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let text = &open[uri];
                let at = offset(
                    text,
                    params["position"]["line"].as_u64().unwrap(),
                    params["position"]["character"].as_u64().unwrap(),
                );
                let result = ident_at(text, at).and_then(|(s, e)| {
                    let name = &text[s..e];
                    text.lines()
                        .map(str::trim)
                        .find(|l| {
                            l.starts_with(&format!("var {name} "))
                                || l.starts_with(&format!("func {name}("))
                        })
                        .map(|l| json!({"contents": {"kind": "markdown", "value": format!("```go\n{}\n```", l.trim_end_matches('{').trim())}}))
                });
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": result}),
                );
            }
            ("textDocument/definition", Some(id)) => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let text = &open[uri];
                let at = offset(
                    text,
                    params["position"]["line"].as_u64().unwrap(),
                    params["position"]["character"].as_u64().unwrap(),
                );
                let result = ident_at(text, at).and_then(|(s, e)| {
                    let name = &text[s..e];
                    let qualifier = text[..s]
                        .strip_suffix('.')
                        .and_then(|before| ident_at(before, before.len()))
                        .map(|(qs, qe)| text[qs..qe].to_string());
                    let here = uri_path(uri);
                    let dir = match &qualifier {
                        Some(q) => root.join(q),
                        None => here.parent().unwrap().to_path_buf(),
                    };
                    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                        .ok()?
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.is_file())
                        .collect();
                    files.sort();
                    files.into_iter().find_map(|file| {
                        let content = std::fs::read_to_string(&file).ok()?;
                        let i = content.find(&format!("func {name}("))? + "func ".len();
                        Some(json!({"uri": format!("file://{}", file.display()),
                            "range": {"start": position(&content, i), "end": position(&content, i + name.len())}}))
                    })
                });
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": result}),
                );
            }
            ("textDocument/prepareCallHierarchy", Some(id)) => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let here = uri_path(uri);
                let text = &open[uri];
                let at = offset(
                    text,
                    params["position"]["line"].as_u64().unwrap(),
                    params["position"]["character"].as_u64().unwrap(),
                );
                // At a declaration, that function; at a call, its callee.
                let result = ident_at(text, at).and_then(|(s, e)| {
                    let name = &text[s..e];
                    if text[..s].ends_with("func ") {
                        return Some(json!([item(&here, text, name, s, 12)]));
                    }
                    let qualifier = text[..s]
                        .strip_suffix('.')
                        .and_then(|before| ident_at(before, before.len()))
                        .map(|(qs, qe)| text[qs..qe].to_string());
                    declaration(&open, &root, &here, qualifier.as_deref(), name).map(|i| json!([i]))
                });
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": result}),
                );
            }
            ("callHierarchy/outgoingCalls", Some(id)) => {
                let file = uri_path(params["item"]["uri"].as_str().unwrap());
                let name = params["item"]["name"].as_str().unwrap();
                let text = text_of(&open, &file).unwrap_or_default();
                let calls: Vec<Value> = funcs(&text)
                    .into_iter()
                    .find(|(n, _, _)| n == name)
                    .map_or_default(|(_, _, body)| calls_in(&text, body))
                    .into_iter()
                    .filter_map(|(q, f)| declaration(&open, &root, &file, q.as_deref(), &f))
                    .map(|to| json!({"to": to, "fromRanges": []}))
                    .collect();
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": calls}),
                );
            }
            ("callHierarchy/incomingCalls", Some(id)) => {
                let target = (
                    params["item"]["uri"].as_str().unwrap().to_string(),
                    params["item"]["name"].as_str().unwrap().to_string(),
                );
                let mut callers = Vec::new();
                for file in go_files(&root) {
                    let Some(text) = text_of(&open, &file) else {
                        continue;
                    };
                    for (name, at, body) in funcs(&text) {
                        let calls_it = calls_in(&text, body).into_iter().any(|(q, f)| {
                            declaration(&open, &root, &file, q.as_deref(), &f).is_some_and(|d| {
                                d["uri"].as_str() == Some(target.0.as_str())
                                    && d["name"].as_str() == Some(target.1.as_str())
                            })
                        });
                        if calls_it {
                            callers.push(json!({"from": item(&file, &text, &name, at, 12), "fromRanges": []}));
                        }
                    }
                }
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": callers}),
                );
            }
            ("textDocument/prepareTypeHierarchy", Some(id)) => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let text = &open[uri];
                let at = offset(
                    text,
                    params["position"]["line"].as_u64().unwrap(),
                    params["position"]["character"].as_u64().unwrap(),
                );
                let result = ident_at(text, at)
                    .filter(|(s, _)| text[..*s].ends_with("type "))
                    .map(|(s, e)| json!([item(&uri_path(uri), text, &text[s..e], s, 23)]));
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": result}),
                );
            }
            ("textDocument/implementation", Some(id)) => {
                // At `type I interface`: every type whose `// implements:`
                // comment names `<package>.I`.
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let here = uri_path(uri);
                let text = &open[uri];
                let at = offset(
                    text,
                    params["position"]["line"].as_u64().unwrap(),
                    params["position"]["character"].as_u64().unwrap(),
                );
                let package = here
                    .parent()
                    .and_then(Path::file_name)
                    .map_or_default(|n| n.to_string_lossy().into_owned());
                let mut found = Vec::new();
                if let Some((s, e)) = ident_at(text, at) {
                    let wanted = format!("{package}.{}", &text[s..e]);
                    for file in go_files(&root) {
                        let Some(t) = text_of(&open, &file) else {
                            continue;
                        };
                        let mut line_start = 0;
                        for line in t.split_inclusive('\n') {
                            let names_it = line.starts_with("type ")
                                && line.split_once("// implements:").is_some_and(|(_, list)| {
                                    list.split(',').any(|q| q.trim() == wanted)
                                });
                            if names_it
                                && let Some((ns, ne)) = ident_at(&t, line_start + "type ".len())
                            {
                                found.push(json!({"uri": format!("file://{}", file.display()),
                                    "range": {"start": position(&t, ns), "end": position(&t, ne)}}));
                            }
                            line_start += line.len();
                        }
                    }
                }
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": found}),
                );
            }
            ("typeHierarchy/supertypes", Some(id)) => {
                let file = uri_path(params["item"]["uri"].as_str().unwrap());
                let name = params["item"]["name"].as_str().unwrap();
                let text = text_of(&open, &file).unwrap_or_default();
                let named = text
                    .lines()
                    .find(|l| l.starts_with(&format!("type {name} ")))
                    .and_then(|l| l.split_once("// implements:"))
                    .map_or_default(|(_, list)| list.to_string());
                let supers: Vec<Value> = named
                    .split(',')
                    .filter_map(|q| q.trim().split_once('.'))
                    .filter_map(|(pkg, iface)| {
                        go_files(&root)
                            .into_iter()
                            .filter(|f| f.parent() == Some(root.join(pkg).as_path()))
                            .find_map(|f| {
                                let t = text_of(&open, &f)?;
                                let at =
                                    t.find(&format!("type {iface} interface"))? + "type ".len();
                                Some(item(&f, &t, iface, at, 11))
                            })
                    })
                    .collect();
                send(
                    &mut out,
                    json!({"jsonrpc": "2.0", "id": id, "result": supers}),
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
                let ext = uri_path(uri)
                    .extension()
                    .map(std::borrow::ToOwned::to_owned);
                let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
                    .unwrap()
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().map(std::borrow::ToOwned::to_owned) == ext)
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
                let ext = Path::new(&uri_path(uri))
                    .extension()
                    .map(std::borrow::ToOwned::to_owned);
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
                    let ext = old.extension().map(std::borrow::ToOwned::to_owned);
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
