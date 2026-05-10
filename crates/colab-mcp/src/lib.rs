//! Model Context Protocol (MCP) server for colab.
//!
//! Wraps the same operations as the CLI as four MCP tools so an
//! agent in Claude Code (or any MCP-aware host) can call them
//! directly:
//!
//! - `colab.schema` — capability discovery (no input).
//! - `colab.lint_script` — parse + compile a script without running.
//! - `colab.preview` — apply a script to one or more paths and
//!   return a unified diff per file.
//! - `colab.apply` — same, but write back to disk.
//!
//! ## Wire format
//!
//! JSON-RPC 2.0 framed with LSP-style `Content-Length` headers over
//! stdio. The supported methods are:
//!
//! - `initialize`
//! - `initialized` (notification, no response)
//! - `tools/list`
//! - `tools/call`
//!
//! Anything else returns the standard JSON-RPC `-32601 method not
//! found` error. The CLI is the source of truth for behaviour; this
//! crate is one more frontend.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;

use colab_core::{BackendRegistry, walker};
use colab_dsl::compile;
use serde_json::{Map, Value, json};

mod tools;

/// JSON-RPC error code for "method not found" (-32601).
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC error code for "invalid params" (-32602).
const INVALID_PARAMS: i64 = -32602;
/// JSON-RPC error code for "internal error" (-32603).
const INTERNAL_ERROR: i64 = -32603;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the MCP server reading from `reader` and writing to
/// `writer`. Returns when the client closes stdin (EOF) or sends an
/// `exit` notification.
pub fn serve<R, W>(reader: R, writer: W, backends: BackendRegistry) -> io::Result<()>
where
    R: Read,
    W: Write,
{
    let mut reader = BufReader::new(reader);
    let mut writer = writer;

    loop {
        let Some(message) = read_message(&mut reader)? else {
            // EOF: client closed stdin.
            return Ok(());
        };
        if message
            .get("method")
            .and_then(|m| m.as_str())
            .map(|m| m == "exit")
            .unwrap_or(false)
        {
            return Ok(());
        }
        handle_streaming(&message, &backends, &mut writer)?;
    }
}

/// Convenience entry point used by the binary: reads stdin, writes
/// stdout, locks both for the duration.
pub fn run(backends: BackendRegistry) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let in_lock = stdin.lock();
    let out_lock = stdout.lock();
    serve(in_lock, out_lock, backends)
}

/// Read a single Content-Length-framed JSON-RPC message. Returns
/// `Ok(None)` on EOF.
fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().ok();
        }
        // Unknown headers (Content-Type, etc.) are silently ignored,
        // matching how the LSP/MCP framing is forgiving.
    }

    let len = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    let value: Value = serde_json::from_slice(&buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(value))
}

/// Write a single Content-Length-framed JSON-RPC message.
fn write_message<W: Write>(writer: &mut W, value: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

/// Dispatch one parsed JSON-RPC message and return the response (if
/// any — notifications produce `None`).
pub fn handle(message: &Value, backends: &BackendRegistry) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(|m| m.as_str())?;

    match method {
        "initialize" => Some(make_response(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "colab-mcp",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        )),
        // `initialized` is a notification: client tells server that
        // initialization is complete. No response is expected.
        "initialized" | "notifications/initialized" => None,
        "tools/list" => Some(make_response(id, json!({ "tools": tools::list() }))),
        "tools/call" => Some(handle_call(id, message.get("params"), backends)),
        // Notifications: no `id`, no response.
        _ if id.is_none() => None,
        _ => Some(make_error(id, METHOD_NOT_FOUND, "method not found")),
    }
}

fn handle_call(id: Option<Value>, params: Option<&Value>, backends: &BackendRegistry) -> Value {
    let Some(params) = params else {
        return make_error(id, INVALID_PARAMS, "missing params");
    };
    let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
        return make_error(id, INVALID_PARAMS, "missing tool name");
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match call_tool(name, &arguments, backends) {
        Ok(content) => make_response(
            id,
            json!({
                "content": [{ "type": "text", "text": content }],
                "isError": false,
            }),
        ),
        Err(err) => make_error(id, INTERNAL_ERROR, &err),
    }
}

/// Streaming dispatcher used by [`serve`]. Inspects `tools/call`
/// requests for an `_meta.progressToken` and, when present, runs
/// `colab.preview` / `colab.apply` with `notifications/progress`
/// emission interleaved with the final response. All other paths
/// (no token, non-streaming tools, other JSON-RPC methods) fall
/// through to the existing synchronous [`handle`].
pub fn handle_streaming<W: Write>(
    message: &Value,
    backends: &BackendRegistry,
    writer: &mut W,
) -> io::Result<()> {
    let is_tools_call = message
        .get("method")
        .and_then(|m| m.as_str())
        .map(|m| m == "tools/call")
        .unwrap_or(false);

    if is_tools_call
        && let Some(params) = message.get("params")
        && let Some(token) = params
            .get("_meta")
            .and_then(|m| m.get("progressToken"))
            .cloned()
    {
        let id = message.get("id").cloned();
        let response = handle_call_with_progress(id, params, backends, writer, &token)?;
        write_message(writer, &response)?;
        return Ok(());
    }

    if let Some(response) = handle(message, backends) {
        write_message(writer, &response)?;
    }
    Ok(())
}

fn handle_call_with_progress<W: Write>(
    id: Option<Value>,
    params: &Value,
    backends: &BackendRegistry,
    writer: &mut W,
    token: &Value,
) -> io::Result<Value> {
    let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
        return Ok(make_error(id, INVALID_PARAMS, "missing tool name"));
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // Only preview/apply emit progress; the others are fast and
    // fall through to the synchronous path.
    let mode = match name {
        "colab.preview" => RunMode::Preview,
        "colab.apply" => RunMode::Apply,
        _ => return Ok(handle_call(id, Some(params), backends)),
    };

    match run_script_with_progress(&arguments, backends, mode, writer, token) {
        Ok(value) => Ok(make_response(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&value).unwrap(),
                }],
                "isError": false,
            }),
        )),
        Err(err) => Ok(make_error(id, INTERNAL_ERROR, &err)),
    }
}

/// Like [`run_script`], but emits a `notifications/progress`
/// message every [`PROGRESS_BATCH`] files plus a final 100%
/// notification before the response is written.
fn run_script_with_progress<W: Write>(
    args: &Value,
    backends: &BackendRegistry,
    mode: RunMode,
    writer: &mut W,
    token: &Value,
) -> Result<Value, String> {
    let script = arg_str(args, "script")?;
    let paths = arg_paths(args, "paths")?;

    let refactoring = compile(&script, backends).map_err(|e| e.to_string())?;

    let mut results: Vec<Value> = Vec::new();
    let mut files_processed: u64 = 0;
    let mut last_emitted: u64 = 0;

    for target in &paths {
        walker::walk(&refactoring, target, &mut |change| {
            let mut entry = Map::new();
            entry.insert("path".into(), json!(change.path.to_string_lossy()));
            entry.insert("changed".into(), json!(change.changed()));
            entry.insert("bytes_before".into(), json!(change.before.len()));
            entry.insert("bytes_after".into(), json!(change.after.len()));
            if change.changed() {
                if matches!(mode, RunMode::Apply) {
                    std::fs::write(&change.path, &change.after)
                        .map_err(|e| colab_core::Error::io_at(&change.path, e))?;
                }
                let diff = unified_diff(&change.path, &change.before, &change.after);
                entry.insert("diff".into(), json!(diff));
            }
            results.push(Value::Object(entry));
            files_processed += 1;
            if files_processed - last_emitted >= PROGRESS_BATCH {
                last_emitted = files_processed;
                // Errors writing a notification are non-fatal —
                // the client may have closed early; the response
                // attempt below will surface a real failure.
                let _ = emit_progress(writer, token, files_processed, None);
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    }

    // Final 100% notification.
    let _ = emit_progress(writer, token, files_processed, Some(files_processed));
    Ok(json!({ "results": results }))
}

/// One progress notification per N files. Hand-tuned to be small
/// enough that 1k-file runs emit ≥10 ticks but not so frequent
/// that the wire becomes the bottleneck.
const PROGRESS_BATCH: u64 = 64;

fn emit_progress<W: Write>(
    writer: &mut W,
    token: &Value,
    progress: u64,
    total: Option<u64>,
) -> io::Result<()> {
    let mut params = Map::new();
    params.insert("progressToken".into(), token.clone());
    params.insert("progress".into(), json!(progress));
    if let Some(total) = total {
        params.insert("total".into(), json!(total));
    }
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": Value::Object(params),
    });
    write_message(writer, &notification)
}

fn call_tool(name: &str, args: &Value, backends: &BackendRegistry) -> Result<String, String> {
    match name {
        "colab.schema" => {
            let value = colab_schema_json(backends);
            Ok(serde_json::to_string_pretty(&value).expect("schema JSON"))
        }
        "colab.lint_script" => {
            let script = arg_str(args, "script")?;
            match compile(&script, backends) {
                Ok(refactoring) => Ok(serde_json::to_string_pretty(&json!({
                    "ok": true,
                    "name": refactoring.name,
                    "rule_count": refactoring.len(),
                }))
                .unwrap()),
                Err(err) => Ok(serde_json::to_string_pretty(&json!({
                    "ok": false,
                    "error": err.to_string(),
                    "exit_code": err.exit_code(),
                }))
                .unwrap()),
            }
        }
        "colab.preview" => {
            let outcome = run_script(args, backends, RunMode::Preview)?;
            Ok(serde_json::to_string_pretty(&outcome).unwrap())
        }
        "colab.apply" => {
            let outcome = run_script(args, backends, RunMode::Apply)?;
            Ok(serde_json::to_string_pretty(&outcome).unwrap())
        }
        other => Err(format!("unknown tool: {}", other)),
    }
}

#[derive(Clone, Copy)]
enum RunMode {
    Preview,
    Apply,
}

fn run_script(
    args: &Value,
    backends: &BackendRegistry,
    mode: RunMode,
) -> Result<Value, String> {
    let script = arg_str(args, "script")?;
    let paths = arg_paths(args, "paths")?;

    let refactoring = compile(&script, backends).map_err(|e| e.to_string())?;

    let mut results: Vec<Value> = Vec::new();
    for target in &paths {
        walker::walk(&refactoring, target, &mut |change| {
            let mut entry = Map::new();
            entry.insert("path".into(), json!(change.path.to_string_lossy()));
            entry.insert("changed".into(), json!(change.changed()));
            entry.insert("bytes_before".into(), json!(change.before.len()));
            entry.insert("bytes_after".into(), json!(change.after.len()));
            if change.changed() {
                if matches!(mode, RunMode::Apply) {
                    std::fs::write(&change.path, &change.after)
                        .map_err(|e| colab_core::Error::io_at(&change.path, e))?;
                }
                let diff = unified_diff(&change.path, &change.before, &change.after);
                entry.insert("diff".into(), json!(diff));
            }
            results.push(Value::Object(entry));
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(json!({ "results": results }))
}

fn unified_diff(path: &std::path::Path, before: &str, after: &str) -> String {
    let display = path.display();
    let header_a = format!("a/{}", display);
    let header_b = format!("b/{}", display);
    similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .header(&header_a, &header_b)
        .to_string()
}

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("missing string argument: {}", key))
}

fn arg_paths(args: &Value, key: &str) -> Result<Vec<PathBuf>, String> {
    let value = args
        .get(key)
        .ok_or_else(|| format!("missing argument: {}", key))?;
    let array = value
        .as_array()
        .ok_or_else(|| format!("argument `{}` must be an array of strings", key))?;
    let mut out = Vec::with_capacity(array.len());
    for item in array {
        let s = item
            .as_str()
            .ok_or_else(|| format!("argument `{}` must contain strings", key))?;
        out.push(PathBuf::from(s));
    }
    if out.is_empty() {
        return Err(format!("argument `{}` cannot be empty", key));
    }
    Ok(out)
}

/// Build the same JSON document `colab schema` emits.
fn colab_schema_json(backends: &BackendRegistry) -> Value {
    let langs: Vec<Value> = backends
        .languages()
        .iter()
        .filter_map(|lang| backends.get(lang).map(language_capabilities))
        .collect();
    json!({ "languages": langs })
}

fn language_capabilities(backend: &dyn colab_core::LanguageBackend) -> Value {
    let modules: Vec<Value> = backend
        .capabilities()
        .iter()
        .map(|cap| {
            let actions: Vec<Value> = cap
                .actions
                .iter()
                .map(|act| json!({ "name": act.name, "description": act.description }))
                .collect();
            json!({
                "name": cap.module,
                "description": cap.description,
                "actions": actions,
            })
        })
        .collect();
    json!({
        "name": backend.lang(),
        "description": backend.description(),
        "modules": modules,
    })
}

fn make_response(id: Option<Value>, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "result": result,
    })
}

fn make_error(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> BackendRegistry {
        let mut r = BackendRegistry::new();
        r.register(Box::new(colab_lang_go::GoBackend));
        r.register(Box::new(colab_lang_rust::RustBackend));
        r
    }

    #[test]
    fn initialize_returns_protocol_version_and_tool_capability() {
        let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn initialized_notification_has_no_response() {
        let req = json!({"jsonrpc":"2.0","method":"initialized"});
        assert!(handle(&req, &registry()).is_none());
        let req2 = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert!(handle(&req2, &registry()).is_none());
    }

    #[test]
    fn tools_list_includes_all_four_tools() {
        let req = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
        let resp = handle(&req, &registry()).unwrap();
        let names: Vec<&str> = resp["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"colab.schema"));
        assert!(names.contains(&"colab.lint_script"));
        assert!(names.contains(&"colab.preview"));
        assert!(names.contains(&"colab.apply"));
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let req = json!({"jsonrpc":"2.0","id":3,"method":"nope"});
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn schema_tool_returns_languages_object() {
        let req = json!({
            "jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"colab.schema","arguments":{}}
        });
        let resp = handle(&req, &registry()).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert!(parsed["languages"].is_array());
    }

    #[test]
    fn lint_script_reports_ok_for_valid_script() {
        let req = json!({
            "jsonrpc":"2.0","id":5,"method":"tools/call",
            "params":{"name":"colab.lint_script","arguments":{
                "script":"refactor \"x\" { match go::import \"a\" { replace \"b\" } }"
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["name"], "x");
        assert_eq!(parsed["rule_count"], 1);
    }

    #[test]
    fn lint_script_reports_error_for_unsupported_namespace() {
        let req = json!({
            "jsonrpc":"2.0","id":6,"method":"tools/call",
            "params":{"name":"colab.lint_script","arguments":{
                "script":"refactor \"x\" { match klingon::module \"a\" { replace \"b\" } }"
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["exit_code"], 3);
    }

    #[test]
    fn lint_script_rejects_missing_script_argument() {
        let req = json!({
            "jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"colab.lint_script","arguments":{}}
        });
        let resp = handle(&req, &registry()).unwrap();
        // Tool-level error: returns an `error` field at JSON-RPC level
        assert!(resp.get("error").is_some(), "got: {resp}");
    }

    #[test]
    fn preview_returns_diff_without_writing() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!(
            "colab-mcp-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let go_path = dir.join("main.go");
        let original = "package main\nimport \"old.module\"\n";
        fs::write(&go_path, original).unwrap();

        let req = json!({
            "jsonrpc":"2.0","id":8,"method":"tools/call",
            "params":{"name":"colab.preview","arguments":{
                "script":"refactor \"r\" { match go::import \"old.module\" { replace \"new.module\" } }",
                "paths":[go_path.to_string_lossy()],
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        let entry = &parsed["results"][0];
        assert_eq!(entry["changed"], true);
        assert!(entry["diff"].as_str().unwrap().contains("--- a/"));
        // File on disk untouched.
        assert_eq!(fs::read_to_string(&go_path).unwrap(), original);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_writes_file() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!(
            "colab-mcp-apply-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let go_path = dir.join("main.go");
        fs::write(&go_path, "package main\nimport \"old.module\"\n").unwrap();

        let req = json!({
            "jsonrpc":"2.0","id":9,"method":"tools/call",
            "params":{"name":"colab.apply","arguments":{
                "script":"refactor \"r\" { match go::import \"old.module\" { replace \"new.module\" } }",
                "paths":[go_path.to_string_lossy()],
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["result"]["isError"], false);
        let on_disk = fs::read_to_string(&go_path).unwrap();
        assert!(on_disk.contains("new.module"));
        assert!(!on_disk.contains("old.module"));
        fs::remove_dir_all(&dir).ok();
    }

    /// Parse a buffer of `Content-Length`-framed JSON-RPC messages
    /// into a `Vec<Value>` for assertions.
    fn parse_messages(buf: &[u8]) -> Vec<Value> {
        let text = std::str::from_utf8(buf).unwrap();
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(header_end) = rest.find("\r\n\r\n") {
            let header = &rest[..header_end];
            let length: usize = header
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length:"))
                .and_then(|v| v.trim().parse().ok())
                .expect("Content-Length");
            let body_start = header_end + 4;
            let body = &rest[body_start..body_start + length];
            out.push(serde_json::from_str(body).unwrap());
            rest = &rest[body_start + length..];
        }
        out
    }

    #[test]
    fn handle_streaming_emits_progress_notifications_when_token_present() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!(
            "colab-mcp-progress-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        // Create > PROGRESS_BATCH (64) Go files so we get at least
        // one mid-walk progress notification.
        for i in 0..80 {
            let p = dir.join(format!("dir{}/{:03}.go", i % 4, i));
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, "package demo\nimport \"old.module\"\n").unwrap();
        }

        let req = json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "tools/call",
            "params": {
                "name": "colab.preview",
                "arguments": {
                    "script": "refactor \"r\" { match go::import \"old.module\" { replace \"new.module\" } }",
                    "paths": [dir.to_string_lossy()],
                },
                "_meta": {
                    "progressToken": "tok-1"
                }
            }
        });

        let mut buf: Vec<u8> = Vec::new();
        handle_streaming(&req, &registry(), &mut buf).unwrap();

        let messages = parse_messages(&buf);
        // At least: ≥1 mid-walk progress + 1 final progress + 1 response.
        let progress: Vec<&Value> = messages
            .iter()
            .filter(|m| {
                m.get("method")
                    .and_then(|x| x.as_str())
                    .map(|s| s == "notifications/progress")
                    .unwrap_or(false)
            })
            .collect();
        let responses: Vec<&Value> = messages.iter().filter(|m| m.get("id").is_some()).collect();

        assert!(progress.len() >= 2, "got {} progress messages", progress.len());
        assert_eq!(responses.len(), 1, "got {:?}", responses);
        assert_eq!(responses[0]["id"], 42);
        // Each progress carries the original token.
        for p in &progress {
            assert_eq!(p["params"]["progressToken"], json!("tok-1"));
        }
        // The final progress carries `total == progress`.
        let last = progress.last().unwrap();
        assert_eq!(last["params"]["progress"], last["params"]["total"]);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn handle_streaming_skips_progress_when_no_token() {
        let req = json!({
            "jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"colab.schema","arguments":{}}
        });
        let mut buf: Vec<u8> = Vec::new();
        handle_streaming(&req, &registry(), &mut buf).unwrap();
        let messages = parse_messages(&buf);
        // Exactly one response, no progress notifications.
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["id"], 7);
        assert!(messages[0].get("method").is_none());
    }

    #[test]
    fn read_write_roundtrip() {
        let mut input = Vec::new();
        let body = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}";
        input.extend_from_slice(b"Content-Length: ");
        input.extend_from_slice(body.len().to_string().as_bytes());
        input.extend_from_slice(b"\r\n\r\n");
        input.extend_from_slice(body);

        let mut output: Vec<u8> = Vec::new();
        serve(input.as_slice(), &mut output, registry()).unwrap();

        // Output is a Content-Length-framed JSON message.
        let text = String::from_utf8(output).unwrap();
        assert!(text.starts_with("Content-Length: "));
        let body_start = text.find("\r\n\r\n").unwrap() + 4;
        let body_json: Value = serde_json::from_str(&text[body_start..]).unwrap();
        assert_eq!(body_json["id"], 1);
        assert!(body_json["result"]["tools"].is_array());
    }
}
