//! Model Context Protocol (MCP) server for colab.
//!
//! Wraps the same operations as the CLI as MCP tools so an agent in
//! Claude Code (or any MCP-aware host) can call them directly:
//!
//! - `colab.list_languages` — the registered backends, by name.
//! - `colab.list_rules` — one backend's modules and actions.
//! - `colab.schema` — the full capability schema, optionally for one
//!   language.
//! - `colab.lint_script` — parse + compile a script without running.
//! - `colab.preview` — apply a script to one or more paths and report
//!   what would change.
//! - `colab.apply` — same, but write back to disk.
//!
//! ## Response shape
//!
//! `preview` / `apply` return aggregate counters, per-rule match counts,
//! and the changed paths — with diffs only when asked for:
//!
//! ```json
//! {"summary":{"visited":412,"scanned":38,"changed":3,"skipped":0,"elapsed_ms":84},
//!  "rules":[{"i":0,"rule":"go::import \"a\" -> \"b\"","files":3},
//!           {"i":1,"rule":"go::symbol \"X\" -> \"Y\"","files":0}],
//!  "changed":["cmd/main.go"],
//!  "applied":false,
//!  "warnings":["rule 2 matched no files: go::symbol \"X\" -> \"Y\""]}
//! ```
//!
//! Files that were scanned but unchanged never appear. The three
//! counters in `summary` distinguish the three ways a run can come back
//! empty (nothing walked / nothing of that language / nothing matched),
//! which one flat list of results cannot.
//!
//! ## Errors
//!
//! A tool that ran and rejected its input returns `isError: true` with
//! `{"error": {kind, message, exit_code, …}}` — including `line`,
//! `column`, and `expected` for a parse failure. JSON-RPC error
//! responses are reserved for malformed *calls* (unknown method or tool,
//! missing or mistyped arguments).
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
use std::path::{Path, PathBuf};
use std::time::Instant;

use colab_core::render::{self, ChangeCollector, Detail, RenderOptions};
use colab_core::report::RunReport;
use colab_core::suggest;
use colab_core::{BackendRegistry, walker};
use colab_dsl::compile;
use serde_json::{Map, Value, json};

mod tools;

/// JSON-RPC error code for "method not found" (-32601).
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC error code for "invalid params" (-32602).
///
/// This is the only error code tool calls produce. A tool that *ran* and
/// rejected its input reports `isError: true` with a structured body
/// instead — `-32603 INTERNAL_ERROR` would claim a server fault for what
/// is really a bad script.
const INVALID_PARAMS: i64 = -32602;

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
    tool_response(id, call_tool(name, &arguments, backends))
}

/// Turn a tool outcome into a `tools/call` response.
///
/// Failures come back as `isError: true` with a structured payload
/// rather than a JSON-RPC error, so a caller sees the same shape however
/// the tool failed and can read `line`/`column`/`expected` directly.
/// JSON-RPC errors are reserved for envelope problems (bad method, bad
/// params) — things that are wrong with the *call*, not the script.
fn tool_response(id: Option<Value>, outcome: Result<Value, ToolError>) -> Value {
    match outcome {
        Ok(value) => make_response(
            id,
            json!({
                "content": [{ "type": "text", "text": compact(&value) }],
                "isError": false,
            }),
        ),
        Err(ToolError::Protocol(message)) => make_error(id, INVALID_PARAMS, &message),
        Err(ToolError::Tool(payload)) => make_response(
            id,
            json!({
                "content": [{ "type": "text", "text": compact(&json!({ "error": payload })) }],
                "isError": true,
            }),
        ),
    }
}

/// Compact, not pretty-printed: this JSON is nested inside an MCP text
/// string, so indentation is escaped and paid for twice.
fn compact(value: &Value) -> String {
    serde_json::to_string(value).expect("serializing a serde_json::Value cannot fail")
}

/// How a tool call failed.
enum ToolError {
    /// The request itself was malformed — wrong tool name, missing or
    /// mistyped arguments. Maps to JSON-RPC `-32602`.
    Protocol(String),
    /// The tool ran and rejected its input — a script that will not
    /// parse, a path that does not exist. Maps to `isError: true` with a
    /// structured body.
    Tool(Value),
}

impl ToolError {
    fn protocol(message: impl Into<String>) -> Self {
        ToolError::Protocol(message.into())
    }

    /// Build the structured body for a [`colab_core::Error`], surfacing
    /// the parse position and expected-token list when there is one.
    fn from_error(err: colab_core::Error) -> Self {
        let mut payload = Map::new();
        payload.insert("message".into(), json!(err.to_string()));
        payload.insert("exit_code".into(), json!(err.exit_code()));
        match &err {
            colab_core::Error::Parse(detail) => {
                payload.insert("kind".into(), json!("parse"));
                payload.insert("line".into(), json!(detail.line));
                payload.insert("column".into(), json!(detail.column));
                payload.insert("offset".into(), json!(detail.offset));
                if !detail.expected.is_empty() {
                    payload.insert("expected".into(), json!(detail.expected));
                }
                if let Some(snippet) = &detail.snippet {
                    payload.insert("snippet".into(), json!(snippet));
                }
            }
            colab_core::Error::UnsupportedOperation(_) => {
                payload.insert("kind".into(), json!("unsupported"));
            }
            colab_core::Error::Io { .. } => {
                payload.insert("kind".into(), json!("io"));
            }
            colab_core::Error::Config(_) => {
                payload.insert("kind".into(), json!("config"));
            }
        }
        ToolError::Tool(Value::Object(payload))
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

    let outcome = run_script_with_progress(&arguments, backends, mode, writer, token);
    Ok(tool_response(id, outcome))
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
) -> Result<Value, ToolError> {
    let script = arg_str(args, "script")?;
    let cwd = arg_opt_path(args, "cwd")?;
    let paths = arg_paths(args, "paths", cwd.as_deref())?;
    let options = arg_render_options(args)?;

    let refactoring =
        compile_script(&script, cwd.as_deref(), backends).map_err(ToolError::from_error)?;

    let started = Instant::now();
    let mut report = RunReport::with_rules(refactoring.rules.iter().map(|r| r.to_string()));
    let mut collector = ChangeCollector::new(options);
    let mut files_processed: u64 = 0;
    let mut last_emitted: u64 = 0;

    for target in &paths {
        let outcome = walker::walk(&refactoring, target, &mut |change| {
            report.record(&change);
            if change.changed() && matches!(mode, RunMode::Apply) {
                std::fs::write(&change.path, &change.after)
                    .map_err(|e| colab_core::Error::io_at(&change.path, e))?;
            }
            collector.push(&change);

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
        .map_err(ToolError::from_error)?;
        report.files_visited += outcome.files_visited;
        report.skipped.extend(outcome.skipped);
    }
    report.elapsed = started.elapsed();

    // Final 100% notification.
    let _ = emit_progress(writer, token, files_processed, Some(files_processed));
    Ok(finish_report(&collector, &report, mode))
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

fn call_tool(name: &str, args: &Value, backends: &BackendRegistry) -> Result<Value, ToolError> {
    match name {
        "colab.list_languages" => Ok(json!({ "languages": backends.languages() })),
        "colab.list_rules" => {
            let lang = arg_str(args, "lang")?;
            let backend = backends.get(&lang).ok_or_else(|| {
                ToolError::from_error(colab_core::Error::UnsupportedOperation(format!(
                    "unknown language `{}`; {}",
                    lang,
                    suggest::candidates_note(&lang, "languages", &backends.languages())
                )))
            })?;
            Ok(language_capabilities(backend))
        }
        "colab.schema" => match args.get("lang").and_then(|v| v.as_str()) {
            Some(lang) => {
                let backend = backends.get(lang).ok_or_else(|| {
                    ToolError::from_error(colab_core::Error::UnsupportedOperation(format!(
                        "unknown language `{}`; {}",
                        lang,
                        suggest::candidates_note(lang, "languages", &backends.languages())
                    )))
                })?;
                Ok(json!({ "languages": [language_capabilities(backend)] }))
            }
            None => Ok(colab_schema_json(backends)),
        },
        "colab.lint_script" => {
            let script = arg_str(args, "script")?;
            let cwd = arg_opt_path(args, "cwd")?;
            let refactoring =
                compile_script(&script, cwd.as_deref(), backends).map_err(ToolError::from_error)?;
            let rules: Vec<String> = refactoring.rules.iter().map(|r| r.to_string()).collect();
            Ok(json!({
                "ok": true,
                "name": refactoring.name,
                "rule_count": refactoring.len(),
                "rules": rules,
            }))
        }
        "colab.preview" => run_script(args, backends, RunMode::Preview),
        "colab.apply" => run_script(args, backends, RunMode::Apply),
        other => Err(ToolError::protocol(format!(
            "unknown tool `{}`; {}",
            other,
            suggest::candidates_note(other, "tools", &TOOL_NAMES)
        ))),
    }
}

const TOOL_NAMES: [&str; 6] = [
    "colab.list_languages",
    "colab.list_rules",
    "colab.schema",
    "colab.lint_script",
    "colab.preview",
    "colab.apply",
];

#[derive(Clone, Copy)]
enum RunMode {
    Preview,
    Apply,
}

/// Compile a script, rooting `include "..."` resolution at `cwd` when
/// one was supplied.
///
/// Without a base path `include` is unusable, and the underlying error
/// names Rust API functions an MCP caller cannot reach — so `cwd` is what
/// makes includes work over this transport at all.
fn compile_script(
    script: &str,
    cwd: Option<&Path>,
    backends: &BackendRegistry,
) -> Result<colab_dsl::Refactoring, colab_core::Error> {
    match cwd {
        // `compile_at_path` resolves includes against the file's parent,
        // so point it at a notional script inside `cwd`.
        Some(dir) => colab_dsl::compile_at_source(script, &dir.join("<script>"), backends),
        None => compile(script, backends),
    }
}

fn run_script(
    args: &Value,
    backends: &BackendRegistry,
    mode: RunMode,
) -> Result<Value, ToolError> {
    let script = arg_str(args, "script")?;
    let cwd = arg_opt_path(args, "cwd")?;
    let paths = arg_paths(args, "paths", cwd.as_deref())?;
    let options = arg_render_options(args)?;

    let refactoring =
        compile_script(&script, cwd.as_deref(), backends).map_err(ToolError::from_error)?;

    let started = Instant::now();
    let mut report = RunReport::with_rules(refactoring.rules.iter().map(|r| r.to_string()));
    let mut collector = ChangeCollector::new(options);

    for target in &paths {
        let outcome = walker::walk(&refactoring, target, &mut |change| {
            report.record(&change);
            if change.changed() && matches!(mode, RunMode::Apply) {
                std::fs::write(&change.path, &change.after)
                    .map_err(|e| colab_core::Error::io_at(&change.path, e))?;
            }
            collector.push(&change);
            Ok(())
        })
        .map_err(ToolError::from_error)?;
        report.files_visited += outcome.files_visited;
        report.skipped.extend(outcome.skipped);
    }
    report.elapsed = started.elapsed();

    Ok(finish_report(&collector, &report, mode))
}

/// Attach the advisories every run should carry: dead rules, and why a
/// run that changed nothing changed nothing.
fn finish_report(collector: &ChangeCollector, report: &RunReport, mode: RunMode) -> Value {
    let mut value = collector.to_json(report);
    let obj = value.as_object_mut().expect("render produces an object");

    obj.insert(
        "applied".into(),
        json!(matches!(mode, RunMode::Apply)),
    );

    let warnings = render::advisories(report);
    if !warnings.is_empty() {
        obj.insert("warnings".into(), json!(warnings));
    }
    value
}

fn arg_str(args: &Value, key: &str) -> Result<String, ToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ToolError::protocol(format!("missing string argument `{}`", key)))
}

fn arg_opt_path(args: &Value, key: &str) -> Result<Option<PathBuf>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(PathBuf::from(s))),
        Some(_) => Err(ToolError::protocol(format!(
            "argument `{}` must be a string",
            key
        ))),
    }
}

fn arg_usize(args: &Value, key: &str, default: usize) -> Result<usize, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| ToolError::protocol(format!("argument `{}` must be a number", key))),
    }
}

fn arg_render_options(args: &Value) -> Result<RenderOptions, ToolError> {
    let detail = match args.get("detail") {
        None | Some(Value::Null) => Detail::default(),
        Some(Value::String(s)) => Detail::parse(s).ok_or_else(|| {
            ToolError::protocol(format!(
                "unknown detail level `{}`; expected one of: {}",
                s,
                Detail::NAMES.join(", ")
            ))
        })?,
        Some(_) => return Err(ToolError::protocol("argument `detail` must be a string")),
    };
    Ok(RenderOptions {
        detail,
        max_files: arg_usize(args, "max_files", render::DEFAULT_MAX_FILES)?,
        max_diff_bytes: arg_usize(args, "max_diff_bytes", render::DEFAULT_MAX_DIFF_BYTES)?,
    })
}

/// Resolve the `paths` argument, rooting relative entries at `cwd`.
fn arg_paths(args: &Value, key: &str, cwd: Option<&Path>) -> Result<Vec<PathBuf>, ToolError> {
    let value = args
        .get(key)
        .ok_or_else(|| ToolError::protocol(format!("missing argument `{}`", key)))?;
    let array = value.as_array().ok_or_else(|| {
        ToolError::protocol(format!("argument `{}` must be an array of strings", key))
    })?;
    let mut out = Vec::with_capacity(array.len());
    for item in array {
        let s = item.as_str().ok_or_else(|| {
            ToolError::protocol(format!("argument `{}` must contain strings", key))
        })?;
        let path = PathBuf::from(s);
        out.push(match cwd {
            Some(dir) if path.is_relative() => dir.join(path),
            _ => path,
        });
    }
    if out.is_empty() {
        return Err(ToolError::protocol(format!(
            "argument `{}` cannot be empty",
            key
        )));
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
        // A failed tool call is flagged as such — not silently `isError:
        // false` with the failure buried in the body.
        assert_eq!(resp["result"]["isError"], true, "got: {resp}");
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["error"]["kind"], "unsupported");
        assert_eq!(parsed["error"]["exit_code"], 3);
        // The message names the languages that do exist.
        let message = parsed["error"]["message"].as_str().unwrap();
        assert!(message.contains("known languages: go, rust"), "{message}");
    }

    #[test]
    fn a_parse_failure_reports_line_column_and_expected_tokens() {
        let req = json!({
            "jsonrpc":"2.0","id":60,"method":"tools/call",
            "params":{"name":"colab.lint_script","arguments":{
                "script":"refactor \"x\" {\n  match go::import \"a\" { replac \"b\" }\n}\n"
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["result"]["isError"], true, "got: {resp}");
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();

        assert_eq!(parsed["error"]["kind"], "parse");
        assert_eq!(parsed["error"]["exit_code"], 2);
        assert_eq!(parsed["error"]["line"], 2);
        assert_eq!(parsed["error"]["column"], 26);
        let expected = parsed["error"]["expected"].as_array().expect("expected list");
        assert!(
            expected.iter().any(|e| e.as_str().unwrap().contains("replace")),
            "got: {expected:?}"
        );
        assert!(parsed["error"]["snippet"].as_str().unwrap().contains('^'));
    }

    #[test]
    fn an_unknown_tool_is_a_protocol_error_that_names_the_real_tools() {
        let req = json!({
            "jsonrpc":"2.0","id":61,"method":"tools/call",
            "params":{"name":"colab.previw","arguments":{}}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["error"]["code"], -32602, "got: {resp}");
        let message = resp["error"]["message"].as_str().unwrap();
        assert!(message.contains("did you mean `colab.preview`?"), "{message}");
    }

    #[test]
    fn list_rules_is_a_fraction_of_the_full_schema() {
        let call = |name: &str, args: Value| {
            let req = json!({
                "jsonrpc":"2.0","id":62,"method":"tools/call",
                "params":{"name": name, "arguments": args}
            });
            let resp = handle(&req, &registry()).unwrap();
            resp["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string()
        };

        let languages = call("colab.list_languages", json!({}));
        let one_lang = call("colab.list_rules", json!({"lang": "go"}));
        let filtered = call("colab.schema", json!({"lang": "go"}));
        let everything = call("colab.schema", json!({}));

        assert!(languages.contains("\"go\""));
        assert!(languages.len() < one_lang.len());
        assert!(one_lang.len() < everything.len());
        assert!(filtered.len() < everything.len());
    }

    #[test]
    fn list_rules_rejects_an_unknown_language_with_a_suggestion() {
        let req = json!({
            "jsonrpc":"2.0","id":63,"method":"tools/call",
            "params":{"name":"colab.list_rules","arguments":{"lang":"rustt"}}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["result"]["isError"], true, "got: {resp}");
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("did you mean `rust`?"), "{text}");
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

        let script =
            "refactor \"r\" { match go::import \"old.module\" { replace \"new.module\" } }";

        // Default detail: counts, no diff.
        let req = json!({
            "jsonrpc":"2.0","id":8,"method":"tools/call",
            "params":{"name":"colab.preview","arguments":{
                "script": script,
                "paths":[go_path.to_string_lossy()],
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["result"]["isError"], false);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();

        assert_eq!(parsed["summary"]["scanned"], 1);
        assert_eq!(parsed["summary"]["changed"], 1);
        assert_eq!(parsed["rules"][0]["files"], 1);
        assert_eq!(parsed["changed"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["applied"], false);
        assert!(parsed.get("diffs").is_none(), "counts must not carry diffs");
        assert!(parsed.get("warnings").is_none(), "got: {parsed}");
        // Compact, not pretty-printed.
        assert!(!text.contains("\n  \""), "response should be compact: {text}");

        // Explicit diff detail adds hunks.
        let req = json!({
            "jsonrpc":"2.0","id":9,"method":"tools/call",
            "params":{"name":"colab.preview","arguments":{
                "script": script,
                "paths":[go_path.to_string_lossy()],
                "detail":"diff",
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert!(
            parsed["diffs"][0]["diff"]
                .as_str()
                .unwrap()
                .contains("--- a/")
        );

        // File on disk untouched by either call.
        assert_eq!(fs::read_to_string(&go_path).unwrap(), original);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_flags_a_dead_rule_without_needing_a_diff() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("colab-mcp-dead-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("main.go"), "package main\nimport \"old.module\"\n").unwrap();

        let req = json!({
            "jsonrpc":"2.0","id":10,"method":"tools/call",
            "params":{"name":"colab.preview","arguments":{
                "script": "refactor \"r\" {\n\
                    match go::import \"old.module\" { replace \"new.module\" }\n\
                    match go::import \"absent.module\" { replace \"x\" }\n\
                  }",
                "paths":[dir.to_string_lossy()],
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();

        assert_eq!(parsed["rules"][0]["files"], 1);
        assert_eq!(parsed["rules"][1]["files"], 0);
        let warnings = parsed["warnings"].as_array().expect("warnings");
        assert!(
            warnings[0].as_str().unwrap().contains("matched no files"),
            "got: {warnings:?}"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn relative_paths_resolve_against_cwd() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("colab-mcp-cwd-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("main.go"), "package main\nimport \"old.module\"\n").unwrap();

        let req = json!({
            "jsonrpc":"2.0","id":11,"method":"tools/call",
            "params":{"name":"colab.preview","arguments":{
                "script":"refactor \"r\" { match go::import \"old.module\" { replace \"new.module\" } }",
                "paths":["main.go"],
                "cwd": dir.to_string_lossy(),
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["result"]["isError"], false, "got: {resp}");
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["summary"]["changed"], 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_result_says_which_kind_of_empty_it_was() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("colab-mcp-empty-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // A file of a language the script does not target.
        fs::write(dir.join("notes.txt"), "nothing to see\n").unwrap();

        let req = json!({
            "jsonrpc":"2.0","id":12,"method":"tools/call",
            "params":{"name":"colab.preview","arguments":{
                "script":"refactor \"r\" { match go::import \"old.module\" { replace \"new.module\" } }",
                "paths":[dir.to_string_lossy()],
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();

        assert_eq!(parsed["summary"]["visited"], 1);
        assert_eq!(parsed["summary"]["scanned"], 0);
        let warnings = parsed["warnings"].as_array().unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| w.as_str().unwrap().contains("none belong to a language")),
            "got: {warnings:?}"
        );
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
