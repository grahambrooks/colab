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
//! - `colab.test_rule` — run a script over one in-memory snippet.
//! - `colab.find` — read-only search: where each rule matches.
//! - `colab.ast` — the syntax tree of a pattern or a snippet.
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
//! JSON-RPC 2.0 over stdio using MCP's stdio transport: one JSON
//! message per line, newline-delimited, with no embedded newlines.
//! (The LSP `colab server` keeps LSP's `Content-Length` framing; that is
//! tower-lsp in colab-cli, not this crate.) A line that is not valid
//! JSON gets a `-32700 parse error` response and the server keeps
//! reading. The supported methods are:
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

use colab_core::BackendRegistry;
use colab_core::render::{self, ChangeCollector, Detail, RenderOptions};
use colab_core::report::RunReport;
use colab_core::suggest;
use colab_core::walker;
use colab_dsl::compile;
use serde_json::{Map, Value, json};

pub mod resources;
mod tools;
mod workflows;

/// JSON-RPC error code for "method not found" (-32601).
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC error code for "parse error" (-32700): a line that is not JSON.
const PARSE_ERROR: i64 = -32700;
/// JSON-RPC error code for "invalid params" (-32602).
///
/// This is the only error code tool calls produce. A tool that *ran* and
/// rejected its input reports `isError: true` with a structured body
/// instead — `-32603 INTERNAL_ERROR` would claim a server fault for what
/// is really a bad script.
const INVALID_PARAMS: i64 = -32602;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// What `initialize` tells the host about when to reach for colab. Hosts
/// such as Claude Code put it in the agent's system prompt, beside the
/// tool names, so it is the one description an agent reads before it
/// decides between colab, its own edit tool and `sed`. Keep it short.
pub const INSTRUCTIONS: &str = "\
colab makes structural, syntax-aware edits (tree-sitter; 12 languages plus TOML, JSON and \
Markdown). Use it instead of sed, perl -pi, a script that rewrites files, or a series of \
single edits whenever the same change repeats: a rename or replacement that `grep -l` finds \
in more than one file, an import path, a call-site shape, a config key. Its matches ignore \
comments and strings, and qualified calls (`svg::to_svg(x)`) match a bare pattern \
(`to_svg($A)`).\n\
\n\
Workflow: colab.find (read-only) or colab.preview to see what a rule touches, then colab.apply. \
Always pass `cwd`. A script can be bare rules:\n\
  match rust::pattern 'serde_yaml::from_str($S)' { replace 'serde_saphyr::from_str($S)' }\n\
  match toml::key \"package.edition\" { set '\"2024\"' }\n\
`$X` binds one node, `$$$X` a sequence; `where` narrows (`where inside 'impl Foo { $$$B }'`). \
A rule reported with `files: 0` matched nothing: fix the rule, do not fall back to sed.\n\
\n\
Also: colab.rename_symbol / colab.rename (scope-aware, via the language server), \
colab.edit_symbol (replace one declaration by name), colab.outline / colab.section / \
colab.edit_markdown (Markdown by heading), colab.set_config_key, colab.move_import. \
colab.schema / colab.list_rules list each language's modules; colab.lint_script checks a script.";

/// Run the MCP server reading from `reader` and writing to
/// `writer`. Returns when the client closes stdin (EOF) or sends an
/// `exit` notification.
pub fn serve<R, W>(reader: R, writer: W, backends: BackendRegistry) -> io::Result<()>
where
    R: Read,
    W: Write,
{
    serve_with(reader, writer, backends, &NoTools)
}

/// Tools contributed by the binary that hosts the server.
///
/// `colab-mcp` depends on `colab-core` and `colab-dsl` only, so tools
/// backed by a language server (`colab.references`, `colab.rename`,
/// `colab.action`) cannot live here. The binary registers them through
/// this trait — the same composition-root pattern as the backend
/// registry — and they are listed and dispatched alongside the built-in
/// ones.
pub trait ToolProvider {
    /// Tool descriptors in the `tools/list` shape (`name`, `description`,
    /// `inputSchema`, `annotations`).
    fn tools(&self) -> Vec<Value>;

    /// Run `name`, or `None` if it is not one of this provider's tools.
    fn call(&self, name: &str, args: &Value) -> Option<colab_core::Result<Value>>;

    /// The checks `colab.preview` / `colab.apply` make over a run's changes
    /// before anything is written, for a run over `roots` — the binary's
    /// Markdown link check, which needs a grammar this crate must not
    /// depend on. None by default.
    fn checks(&self, _roots: &[PathBuf]) -> Vec<Box<dyn colab_dsl::pipeline::Check>> {
        Vec::new()
    }
}

/// No extra tools.
pub struct NoTools;

impl ToolProvider for NoTools {
    fn tools(&self) -> Vec<Value> {
        Vec::new()
    }

    fn call(&self, _name: &str, _args: &Value) -> Option<colab_core::Result<Value>> {
        None
    }
}

/// [`serve`], with the host's extra tools.
pub fn serve_with<R, W>(
    reader: R,
    writer: W,
    backends: BackendRegistry,
    extra: &dyn ToolProvider,
) -> io::Result<()>
where
    R: Read,
    W: Write,
{
    let mut reader = BufReader::new(reader);
    let mut writer = writer;

    loop {
        let message = match read_message(&mut reader)? {
            None => return Ok(()), // EOF: client closed stdin.
            Some(Ok(message)) => message,
            Some(Err(e)) => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": PARSE_ERROR, "message": format!("parse error: {e}") },
                });
                write_message(&mut writer, &response)?;
                continue;
            }
        };
        if message
            .get("method")
            .and_then(|m| m.as_str())
            .is_some_and(|m| m == "exit")
        {
            return Ok(());
        }
        handle_streaming_with(&message, &backends, extra, &mut writer)?;
    }
}

/// Convenience entry point used by the binary: reads stdin, writes
/// stdout, locks both for the duration.
pub fn run(backends: BackendRegistry) -> io::Result<()> {
    run_with(backends, &NoTools)
}

/// [`run`], with the host's extra tools.
pub fn run_with(backends: BackendRegistry, extra: &dyn ToolProvider) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let in_lock = stdin.lock();
    let out_lock = stdout.lock();
    serve_with(in_lock, out_lock, backends, extra)
}

/// Read one newline-delimited JSON-RPC message (MCP stdio transport).
/// Blank lines are skipped. Returns `Ok(None)` on EOF, and
/// `Ok(Some(Err(_)))` for a line that is not valid JSON, so the caller
/// can answer with a parse error instead of dropping the connection.
fn read_message<R: BufRead>(
    reader: &mut R,
) -> io::Result<Option<Result<Value, serde_json::Error>>> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return Ok(Some(serde_json::from_str(trimmed)));
        }
    }
}

/// Write one JSON-RPC message as a single line. `serde_json` emits
/// compact JSON, which escapes every newline inside strings, so the
/// message never spans lines.
fn write_message<W: Write>(writer: &mut W, value: &Value) -> io::Result<()> {
    let mut body = serde_json::to_vec(value)?;
    body.push(b'\n');
    writer.write_all(&body)?;
    writer.flush()
}

/// Dispatch one parsed JSON-RPC message and return the response (if
/// any — notifications produce `None`).
pub fn handle(message: &Value, backends: &BackendRegistry) -> Option<Value> {
    handle_with(message, backends, &NoTools)
}

/// [`handle`], with the host's extra tools.
pub fn handle_with(
    message: &Value,
    backends: &BackendRegistry,
    extra: &dyn ToolProvider,
) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(|m| m.as_str())?;

    match method {
        "initialize" => Some(make_response(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {}, "resources": {} },
                "serverInfo": {
                    "name": "colab-mcp",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "instructions": INSTRUCTIONS,
            }),
        )),
        // `initialized` is a notification: client tells server that
        // initialization is complete. No response is expected.
        "initialized" | "notifications/initialized" => None,
        "tools/list" => {
            let mut list = tools::list();
            list.extend(extra.tools());
            Some(make_response(id, json!({ "tools": list })))
        }
        "tools/call" => Some(handle_call(id, message.get("params"), backends, extra)),
        "resources/list" => Some(make_response(id, resources::list())),
        "resources/read" => {
            let uri = message
                .pointer("/params/uri")
                .and_then(Value::as_str)
                .unwrap_or("");
            Some(match resources::read(uri) {
                Some(result) => make_response(id, result),
                // MCP's "resource not found".
                None => make_error(
                    id,
                    -32002,
                    &format!("no resource `{uri}`; resources/list names them"),
                ),
            })
        }
        // Notifications: no `id`, no response.
        _ if id.is_none() => None,
        _ => Some(make_error(id, METHOD_NOT_FOUND, "method not found")),
    }
}

/// One `tools/call`, as a run of its own: calls never share an oracle,
/// scan facts or an unanswerable question.
fn handle_call(
    id: Option<Value>,
    params: Option<&Value>,
    backends: &BackendRegistry,
    extra: &dyn ToolProvider,
) -> Value {
    let tool = params
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("unnamed");
    // A trace per call: the server itself runs for as long as the client.
    let mut span = colab_core::telemetry::root(format!("colab mcp {tool}"));
    span.set("colab.mcp.tool", tool);
    let arguments = params.and_then(|p| p.get("arguments"));
    let response = call_run(arguments).enter(|| handle_call_in_run(id, params, backends, extra));
    trace_outcome(&mut span, &response);
    response
}

/// A run for one tool call. The tools join relative `paths` onto `cwd`,
/// so the walk yields absolute paths; the call's `cwd` is the run's base,
/// so a rule's `in "<glob>"` reads relative to it, as it does from the
/// CLI's working directory.
fn call_run(arguments: Option<&Value>) -> std::sync::Arc<colab_core::run::Run> {
    let run = colab_core::run::Run::new();
    run.set_base(
        arguments
            .and_then(|a| a.get("cwd"))
            .and_then(Value::as_str)
            .map(PathBuf::from),
    );
    run
}

/// Mark a tool call's span failed when the response says it failed.
fn trace_outcome(span: &mut colab_core::telemetry::Span, response: &Value) {
    if let Some(error) = response.get("error") {
        span.fail(format!("protocol error {}", error["code"]));
    } else if response["result"]["isError"] == true {
        let body: Value = response["result"]["content"][0]["text"]
            .as_str()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default();
        let kind = body["error"]["kind"].as_str().unwrap_or("tool").to_string();
        span.set("error.type", kind.clone());
        span.fail(kind);
    }
}

fn handle_call_in_run(
    id: Option<Value>,
    params: Option<&Value>,
    backends: &BackendRegistry,
    extra: &dyn ToolProvider,
) -> Value {
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
    if let Some(outcome) = extra.call(name, &arguments) {
        return tool_response(id, outcome.map_err(ToolError::from_error));
    }
    tool_response(id, call_tool(name, &arguments, backends, extra))
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
#[derive(Debug)]
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
        payload.insert("kind".into(), json!(err.kind()));
        if let colab_core::Error::Parse(detail) = &err {
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
    handle_streaming_with(message, backends, &NoTools, writer)
}

/// [`handle_streaming`], with the host's extra tools.
pub fn handle_streaming_with<W: Write>(
    message: &Value,
    backends: &BackendRegistry,
    extra: &dyn ToolProvider,
    writer: &mut W,
) -> io::Result<()> {
    let is_tools_call = message
        .get("method")
        .and_then(|m| m.as_str())
        .is_some_and(|m| m == "tools/call");

    if is_tools_call
        && let Some(params) = message.get("params")
        && let Some(token) = params
            .get("_meta")
            .and_then(|m| m.get("progressToken"))
            .cloned()
    {
        let id = message.get("id").cloned();
        let response = handle_call_with_progress(id, params, backends, extra, writer, &token)?;
        write_message(writer, &response)?;
        return Ok(());
    }

    if let Some(response) = handle_with(message, backends, extra) {
        write_message(writer, &response)?;
    }
    Ok(())
}

fn handle_call_with_progress<W: Write>(
    id: Option<Value>,
    params: &Value,
    backends: &BackendRegistry,
    extra: &dyn ToolProvider,
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
        _ => return Ok(handle_call(id, Some(params), backends, extra)),
    };

    let mut sink = Notifications {
        writer,
        token,
        last_emitted: 0,
    };
    let mut span = colab_core::telemetry::root(format!("colab mcp {name}"));
    span.set("colab.mcp.tool", name);
    span.set("colab.mcp.streaming", true);
    let outcome = call_run(Some(&arguments))
        .enter(|| run_script(&arguments, backends, extra, mode, &mut sink));
    let response = tool_response(id, outcome);
    trace_outcome(&mut span, &response);
    Ok(response)
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

fn call_tool(
    name: &str,
    args: &Value,
    backends: &BackendRegistry,
    extra: &dyn ToolProvider,
) -> Result<Value, ToolError> {
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
            let rules: Vec<String> = refactoring
                .rules
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            let mut doc = json!({
                "ok": true,
                "name": refactoring.name,
                "rule_count": refactoring.len(),
                "rules": rules,
            });
            let warnings = refactoring.report().warnings;
            if !warnings.is_empty() {
                doc["warnings"] = json!(warnings);
            }
            Ok(doc)
        }
        "colab.test_rule" => test_rule(args, backends),
        "colab.find" => find(args, backends),
        "colab.ast" => ast(args, backends),
        "colab.preview" => run_script(args, backends, extra, RunMode::Preview, &mut NoProgress),
        "colab.apply" => run_script(args, backends, extra, RunMode::Apply, &mut NoProgress),
        "colab.move_import" => {
            let script = workflows::move_import_script(args, backends)?;
            run_generated(script, args, backends, extra)
        }
        "colab.set_config_key" => {
            let script = workflows::config_key_script(args)?;
            run_generated(script, args, backends, extra)
        }
        "colab.edit_markdown" => {
            let script = workflows::markdown_script(args)?;
            run_generated(script, args, backends, extra)
        }
        other => Err(ToolError::protocol(format!(
            "unknown tool `{}`; {}",
            other,
            suggest::candidates_note(other, "tools", &TOOL_NAMES)
        ))),
    }
}

/// Run a script a workflow tool generated, previewing unless `write` is
/// true, and return the report with the `script` beside it.
fn run_generated(
    script: String,
    args: &Value,
    backends: &BackendRegistry,
    extra: &dyn ToolProvider,
) -> Result<Value, ToolError> {
    let mut run_args = args.clone();
    run_args["script"] = json!(script);
    let mode = if arg_bool(args, "write")? {
        RunMode::Apply
    } else {
        RunMode::Preview
    };
    let mut value = run_script(&run_args, backends, extra, mode, &mut NoProgress)?;
    value["script"] = json!(script);
    Ok(value)
}

const TOOL_NAMES: [&str; 12] = [
    "colab.edit_markdown",
    "colab.move_import",
    "colab.set_config_key",
    "colab.find",
    "colab.ast",
    "colab.list_languages",
    "colab.list_rules",
    "colab.schema",
    "colab.lint_script",
    "colab.test_rule",
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
    // Bare `match …` rules are wrapped as the CLI's `-e` wraps them, and a
    // parse error is reported against the text the caller sent.
    let wrapped = colab_dsl::inline::wrap(script);
    let text = wrapped.as_deref().unwrap_or(script);
    let refactoring = match cwd {
        // `compile_at_path` resolves includes against the file's parent,
        // so point it at a notional script inside `cwd`.
        Some(dir) => colab_dsl::compile_at_source(text, &dir.join("<script>"), backends),
        None => compile(text, backends),
    }
    .map_err(|e| match wrapped {
        Some(_) => colab_dsl::inline::relocate(script, e),
        None => e,
    })?;
    // The MCP server starts no language server, so it cannot answer them.
    if refactoring.needs_oracle() {
        return Err(colab_core::Error::UnsupportedOperation(
            "`type` and `resolves` constraints are answered by a language server, which the MCP \
             server does not run; run the script with the colab CLI (`colab refactor` / `colab find`)"
                .into(),
        ));
    }
    Ok(refactoring)
}

/// A phased run reads every file before it edits one. `colab.preview` and
/// `colab.apply` make one (through `colab_dsl::pipeline`); `tool` reads one
/// snippet, or locates matches file by file, so it refuses the script.
fn refuse_scans(refactoring: &colab_dsl::Refactoring, tool: &str) -> Result<(), ToolError> {
    if !refactoring.has_scans() {
        return Ok(());
    }
    Err(ToolError::from_error(
        colab_core::Error::UnsupportedOperation(format!(
            "a script with `scan` phases reads every file before it edits one, which {tool} \
             does not do yet; run it with colab.preview, or the colab CLI (`colab find`)"
        )),
    ))
}

/// Where per-file progress goes while a script runs.
///
/// The streaming and synchronous entry points differ *only* in this, so
/// modelling it as a sink keeps one implementation of the run itself.
/// When these were two functions they were 61% identical, and a change to
/// the report shape had to be made in both — a divergence would have
/// meant an agent that passes a `progressToken` seeing different
/// behaviour from one that does not.
trait ProgressSink {
    /// Called once per processed file.
    fn tick(&mut self, processed: u64);
    /// Called once when the run finishes, with the final count.
    fn finish(&mut self, total: u64);
}

/// The synchronous path: no notifications.
struct NoProgress;

impl ProgressSink for NoProgress {
    fn tick(&mut self, _processed: u64) {}
    fn finish(&mut self, _total: u64) {}
}

/// The streaming path: a `notifications/progress` message every
/// [`PROGRESS_BATCH`] files, plus a final 100% tick.
struct Notifications<'a, W: Write> {
    writer: &'a mut W,
    token: &'a Value,
    last_emitted: u64,
}

impl<W: Write> ProgressSink for Notifications<'_, W> {
    fn tick(&mut self, processed: u64) {
        if processed - self.last_emitted >= PROGRESS_BATCH {
            self.last_emitted = processed;
            // Errors writing a notification are non-fatal — the client
            // may have closed early; the response attempt will surface a
            // real failure.
            let _ = emit_progress(self.writer, self.token, processed, None);
        }
    }

    fn finish(&mut self, total: u64) {
        let _ = emit_progress(self.writer, self.token, total, Some(total));
    }
}

/// Compile and run a script, reporting progress through `sink`.
///
/// The single implementation behind both `colab.preview`/`colab.apply`
/// entry points. Pass [`NoProgress`] for the synchronous path and
/// [`Notifications`] when the client supplied a `progressToken`.
fn run_script<P: ProgressSink>(
    args: &Value,
    backends: &BackendRegistry,
    extra: &dyn ToolProvider,
    mode: RunMode,
    sink: &mut P,
) -> Result<Value, ToolError> {
    use colab_dsl::pipeline::{Hooks, Pipeline, Source};

    /// Progress ticks per file the walk produces.
    struct Progress<'s, P>(&'s mut P, u64);
    impl<P: ProgressSink> Hooks for Progress<'_, P> {
        fn progress(&mut self, files: u64) {
            self.1 = files;
            self.0.tick(files);
        }
    }

    let script = arg_str(args, "script")?;
    let cwd = arg_opt_path(args, "cwd")?;
    let paths = arg_paths(args, "paths", cwd.as_deref())?;
    let options = arg_render_options(args)?;
    let allow_conflicts = arg_bool(args, "allow_conflicts")?;
    let checks = if arg_bool(args, "no_link_check")? {
        Vec::new()
    } else {
        extra.checks(&paths)
    };

    let refactoring =
        compile_script(&script, cwd.as_deref(), backends).map_err(ToolError::from_error)?;

    let mut progress = Progress(sink, 0);
    let outcome = Pipeline::new(&refactoring, Source::walk(paths))
        .write(matches!(mode, RunMode::Apply))
        .allow_conflicts(allow_conflicts)
        .refusal_hint("fix the rules, or call again with allow_conflicts: true")
        .checks(checks)
        .execute(&mut progress)
        .map_err(ToolError::from_error)?;
    let files_processed = progress.1;
    sink.finish(files_processed);

    let mut collector = ChangeCollector::new(options);
    for change in &outcome.changes {
        collector.push(change);
    }
    let mode = if outcome.blocked {
        RunMode::Preview
    } else {
        mode
    };
    Ok(finish_report(&collector, &outcome.report, mode))
}

/// `colab.test_rule`: run a script over one in-memory snippet.
///
/// The MCP twin of `colab refactor --stdin --path <hint>`. It goes
/// through the same `apply_at` the walker uses, so per-file gating
/// (`is_file_relevant`, `in "<glob>"`) and per-rule attribution behave
/// exactly as they would on disk — a rule that says `files: 0` here will
/// say it in `colab.preview` too.
fn test_rule(args: &Value, backends: &BackendRegistry) -> Result<Value, ToolError> {
    let script = arg_str(args, "script")?;
    let source = arg_str(args, "source")?;
    let path = PathBuf::from(arg_str(args, "path")?);
    let cwd = arg_opt_path(args, "cwd")?;
    let expected = match args.get("expected") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(ToolError::protocol("argument `expected` must be a string")),
    };

    let refactoring =
        compile_script(&script, cwd.as_deref(), backends).map_err(ToolError::from_error)?;
    refuse_scans(&refactoring, "colab.test_rule")?;
    let (change, report) = colab_dsl::pipeline::run_text(&refactoring, path.clone(), source)
        .map_err(ToolError::from_error)?;

    let mut out = Map::new();
    out.insert("changed".into(), json!(change.changed()));
    out.insert("rules".into(), render::rules_json(&report));
    if let Some(conflicts) = render::conflicts_json(&report) {
        out.insert("conflicts".into(), conflicts);
    }
    if change.changed() {
        out.insert(
            "diff".into(),
            json!(render::unified_diff(&path, &change.before, &change.after)),
        );
    }
    if let Some(expected) = expected {
        let matched = expected == change.after;
        out.insert("matches_expected".into(), json!(matched));
        if !matched {
            out.insert(
                "expected_diff".into(),
                json!(render::unified_diff(&path, &change.after, &expected)),
            );
        }
    }

    let mut warnings = render::advisories(&report);
    if !refactoring.rules.iter().any(|r| r.is_file_relevant(&path)) {
        warnings.push(format!(
            "No rule applies to `{}`: check its extension against the languages the script targets, and any `in \"<glob>\"` scope.",
            path.display()
        ));
    }
    if !warnings.is_empty() {
        out.insert("warnings".into(), json!(warnings));
    }
    out.insert("output".into(), json!(change.after));
    Ok(Value::Object(out))
}

/// `colab.find`: read-only search, the twin of `colab find --format json`.
fn find(args: &Value, backends: &BackendRegistry) -> Result<Value, ToolError> {
    use colab_core::find::{FindDetail, FindReport, Hit, Selector};

    let script = arg_str(args, "script")?;
    let cwd = arg_opt_path(args, "cwd")?;
    let paths = arg_paths(args, "paths", cwd.as_deref())?;
    let detail = match args.get("detail") {
        None | Some(Value::Null) => FindDetail::Concise,
        Some(Value::String(s)) => FindDetail::parse(s).ok_or_else(|| {
            ToolError::protocol(format!(
                "argument `detail` must be `concise` or `detailed`, not `{s}`"
            ))
        })?,
        Some(_) => return Err(ToolError::protocol("argument `detail` must be a string")),
    };
    let max_results = arg_usize(args, "max_results", colab_core::find::DEFAULT_MAX_RESULTS)?;

    let refactoring =
        compile_script(&script, cwd.as_deref(), backends).map_err(ToolError::from_error)?;
    refuse_scans(&refactoring, "colab.find")?;
    let rules = &refactoring.rules;
    let mut report = FindReport::new(rules, max_results, detail);
    refactoring.annotate(&mut report.run);
    report.script = refactoring.name.clone();
    let selector = Selector::new(rules, &report.script);
    for target in &paths {
        let outcome = walker::walk(&selector, target, &mut |change| {
            report.record_found(&change.path, selector.take(&change.path));
            Ok(())
        })
        .map_err(ToolError::from_error)?;
        report.run.record_walk(outcome);
    }
    colab_core::near_miss::probe_disk(rules, &mut report.run);

    // Report paths as the caller named them: relative to `cwd`.
    if let Some(cwd) = &cwd {
        report.hits = std::mem::take(&mut report.hits)
            .into_iter()
            .map(|hit| Hit {
                path: hit
                    .path
                    .strip_prefix(cwd)
                    .map(Path::to_path_buf)
                    .unwrap_or(hit.path),
                ..hit
            })
            .collect();
    }
    Ok(render::find_json(&report))
}

/// `colab.ast`: a pattern's or a snippet's syntax tree.
fn ast(args: &Value, backends: &BackendRegistry) -> Result<Value, ToolError> {
    let lang = arg_str(args, "lang")?;
    let backend = backends.get(&lang).ok_or_else(|| {
        ToolError::from_error(colab_core::Error::UnsupportedOperation(format!(
            "unknown language `{}`; {}",
            lang,
            suggest::candidates_note(&lang, "languages", &backends.languages())
        )))
    })?;
    let pattern = args.get("pattern").and_then(|v| v.as_str());
    let source = args.get("source").and_then(|v| v.as_str());
    let (input, is_pattern) = match (pattern, source) {
        (Some(p), None) => (colab_core::AstInput::Pattern(p), true),
        (None, Some(s)) => (colab_core::AstInput::Source(s), false),
        _ => {
            return Err(ToolError::protocol(
                "give exactly one of `pattern` or `source`",
            ));
        }
    };
    let tree = backend.syntax_tree(input).map_err(ToolError::from_error)?;
    let mut out = json!({ "lang": lang, "tree": tree.tree });
    if is_pattern {
        out["context"] = json!(tree.context);
        out["prefilter"] = json!(tree.prefilter);
    } else {
        out["has_error"] = json!(tree.has_error);
    }
    Ok(out)
}

/// Attach the advisories every run should carry: dead rules, and why a
/// run that changed nothing changed nothing.
fn finish_report(collector: &ChangeCollector, report: &RunReport, mode: RunMode) -> Value {
    let mut value = collector.to_json(report);
    let obj = value.as_object_mut().expect("render produces an object");

    obj.insert("applied".into(), json!(matches!(mode, RunMode::Apply)));

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
        .ok_or_else(|| ToolError::protocol(format!("missing string argument `{key}`")))
}

fn arg_bool(args: &Value, key: &str) -> Result<bool, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(ToolError::protocol(format!(
            "argument `{key}` must be a boolean"
        ))),
    }
}

fn arg_opt_path(args: &Value, key: &str) -> Result<Option<PathBuf>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(PathBuf::from(s))),
        Some(_) => Err(ToolError::protocol(format!(
            "argument `{key}` must be a string"
        ))),
    }
}

fn arg_usize(args: &Value, key: &str, default: usize) -> Result<usize, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
            .ok_or_else(|| ToolError::protocol(format!("argument `{key}` must be a number"))),
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
        .ok_or_else(|| ToolError::protocol(format!("missing argument `{key}`")))?;
    let array = value.as_array().ok_or_else(|| {
        ToolError::protocol(format!("argument `{key}` must be an array of strings"))
    })?;
    let mut out = Vec::with_capacity(array.len());
    for item in array {
        let s = item
            .as_str()
            .ok_or_else(|| ToolError::protocol(format!("argument `{key}` must contain strings")))?;
        let path = PathBuf::from(s);
        out.push(match cwd {
            Some(dir) if path.is_relative() => dir.join(path),
            _ => path,
        });
    }
    if out.is_empty() {
        return Err(ToolError::protocol(format!(
            "argument `{key}` cannot be empty"
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

    /// Rewrites `path` on disk at the first tick: someone saving the file
    /// after the walk read it and before the run writes it.
    struct EditsMidRun<'a> {
        path: &'a Path,
        text: &'a str,
    }

    impl ProgressSink for EditsMidRun<'_> {
        fn tick(&mut self, _processed: u64) {
            std::fs::write(self.path, self.text).unwrap();
        }
        fn finish(&mut self, _total: u64) {}
    }

    #[test]
    fn apply_merges_a_file_changed_after_the_walk_read_it() {
        let dir = std::env::temp_dir().join(format!("colab-mcp-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.go");
        let read = "package main\n\nimport \"old/pkg\"\n\nfunc main() {}\n";
        let script = "refactor \"r\" { match go::import \"old/pkg\" { replace \"new/pkg\" } }";
        let arguments = json!({"script": script, "paths": ["."], "cwd": dir.to_string_lossy()});

        // Their edit is far from ours: both survive.
        std::fs::write(&file, read).unwrap();
        let theirs = "package main\n\nimport \"old/pkg\"\n\nfunc main() { run() }\n";
        let mut sink = EditsMidRun {
            path: &file,
            text: theirs,
        };
        let value =
            run_script(&arguments, &registry(), &NoTools, RunMode::Apply, &mut sink).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "package main\n\nimport \"new/pkg\"\n\nfunc main() { run() }\n"
        );
        assert!(value.to_string().contains("merged with the run's changes"));

        // Their edit is on our line: a conflict, and their text stands.
        std::fs::write(&file, read).unwrap();
        let theirs = "package main\n\nimport \"other/pkg\"\n\nfunc main() {}\n";
        let mut sink = EditsMidRun {
            path: &file,
            text: theirs,
        };
        let value =
            run_script(&arguments, &registry(), &NoTools, RunMode::Apply, &mut sink).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), theirs);
        assert!(
            value
                .to_string()
                .contains("changed on disk after colab read it")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn registry() -> BackendRegistry {
        colab_backends::registry()
    }

    #[test]
    fn preview_and_apply_run_a_script_with_scans() {
        let dir = std::env::temp_dir().join(format!("colab-mcp-scan-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("api")).unwrap();
        std::fs::create_dir_all(dir.join("app")).unwrap();
        std::fs::write(
            dir.join("api/api.go"),
            "package api\n\nfunc Exported() {}\n\nfunc internal() {}\n",
        )
        .unwrap();
        let main = dir.join("app/main.go");
        std::fs::write(
            &main,
            "package main\n\nfunc main() {\n\tapi.Exported()\n\tapi.internal()\n}\n",
        )
        .unwrap();
        let script = r#"refactor "s" {
    scan go::pattern 'func $F() {}' where $F matches /^[A-Z]/ as exported
    match go::pattern 'api.$F()' where $F in exported { replace 'api.$F(nil)' }
}"#;
        let arguments = json!({"script": script, "paths": ["."], "cwd": dir.to_string_lossy()});
        let preview = run_script(
            &arguments,
            &registry(),
            &NoTools,
            RunMode::Preview,
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(preview["summary"]["changed"], 1, "{preview}");
        run_script(
            &arguments,
            &registry(),
            &NoTools,
            RunMode::Apply,
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&main).unwrap(),
            "package main\n\nfunc main() {\n\tapi.Exported(nil)\n\tapi.internal()\n}\n"
        );
        // The tools that read file by file still refuse, naming themselves.
        let e = find(&arguments, &registry()).unwrap_err();
        assert!(
            format!("{e:?}").contains("colab.find does not do yet"),
            "{e:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A host whose one check finds a conflict in every changed file.
    struct Objects;

    struct ObjectToAll;

    impl colab_dsl::pipeline::Check for ObjectToAll {
        fn verify(
            &mut self,
            _rules: &[Box<dyn colab_core::Operation>],
            changes: &[colab_core::FileChange],
            report: &mut RunReport,
        ) -> colab_core::Result<()> {
            for change in changes {
                report.conflicts.push(colab_core::report::Conflict {
                    path: change.path.clone(),
                    rule: None,
                    message: "the host objects".into(),
                });
            }
            Ok(())
        }
    }

    impl ToolProvider for Objects {
        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }

        fn call(&self, _name: &str, _args: &Value) -> Option<colab_core::Result<Value>> {
            None
        }

        fn checks(&self, _roots: &[PathBuf]) -> Vec<Box<dyn colab_dsl::pipeline::Check>> {
            vec![Box::new(ObjectToAll)]
        }
    }

    #[test]
    fn apply_runs_the_hosts_checks_before_it_writes() {
        let dir = std::env::temp_dir().join(format!("colab-mcp-checks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.go");
        let text = "package main\n\nimport \"old/pkg\"\n\nfunc main() {}\n";
        std::fs::write(&file, text).unwrap();
        let script = "match go::import \"old/pkg\" { replace \"new/pkg\" }";
        let arguments = json!({"script": script, "paths": ["."], "cwd": dir.to_string_lossy()});

        let value = run_script(
            &arguments,
            &registry(),
            &Objects,
            RunMode::Apply,
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(value["applied"], false, "{value}");
        assert!(value.to_string().contains("the host objects"), "{value}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);

        // `no_link_check` turns the host's checks off.
        let mut skip = arguments.clone();
        skip["no_link_check"] = json!(true);
        let value = run_script(
            &skip,
            &registry(),
            &Objects,
            RunMode::Apply,
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(value["applied"], true, "{value}");
        assert!(std::fs::read_to_string(&file).unwrap().contains("new/pkg"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_rule_that_needs_a_language_server_is_refused() {
        let script = r#"refactor "t" { match go::pattern '$F($$$A)' where $F resolves "alog.Print" { delete } }"#;
        let e = compile_script(script, None, &registry())
            .err()
            .unwrap()
            .to_string();
        assert!(
            e.contains("language server") && e.contains("colab CLI"),
            "{e}"
        );
    }

    #[test]
    fn initialize_returns_protocol_version_and_tool_capability() {
        let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(resp["result"]["capabilities"]["tools"].is_object());
        let instructions = resp["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains("instead of sed"), "{instructions}");
    }

    #[test]
    fn instructions_name_only_tools_the_server_has() {
        // The binary adds the LSP and Markdown tools; name those here.
        let injected = [
            "colab.rename_symbol",
            "colab.rename",
            "colab.edit_symbol",
            "colab.outline",
            "colab.section",
        ];
        let known: Vec<String> = tools::list()
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .chain(injected.iter().map(ToString::to_string))
            .collect();
        for word in INSTRUCTIONS.split(|c: char| !(c.is_alphanumeric() || c == '.' || c == '_')) {
            let word = word.trim_end_matches('.');
            if word.starts_with("colab.") {
                assert!(known.iter().any(|k| k == word), "{word} is not a tool");
            }
        }
    }

    #[test]
    fn a_bare_rule_needs_no_refactor_block() {
        let ok = call_ok(
            "colab.test_rule",
            json!({
                "script": "match rust::pattern 'f($A)' { replace 'g($A)' }",
                "source": "fn m() { f(1); }\n",
                "path": "m.rs",
            }),
        );
        assert_eq!(ok["changed"], true, "{ok}");

        let req = json!({
            "jsonrpc":"2.0","id":41,"method":"tools/call",
            "params":{"name":"colab.test_rule","arguments": {
                "script": "match rust::pattern 'f($A)' { nope }",
                "source": "fn m() {}\n",
                "path": "m.rs",
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["result"]["isError"], true, "{resp}");
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let err: Value = serde_json::from_str(text).unwrap();
        assert_eq!(err["error"]["kind"], "parse", "{err}");
        assert_eq!(err["error"]["line"], 1);
        assert_eq!(
            err["error"]["column"], 31,
            "the column in the text sent, not the wrapper"
        );
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
        assert!(message.contains("known languages:"), "{message}");
        assert!(message.contains("go"), "{message}");
        assert!(message.contains("rust"), "{message}");
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
        let expected = parsed["error"]["expected"]
            .as_array()
            .expect("expected list");
        assert!(
            expected
                .iter()
                .any(|e| e.as_str().unwrap().contains("replace")),
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
        assert!(
            message.contains("did you mean `colab.preview`?"),
            "{message}"
        );
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
        assert!(
            !text.contains("\n  \""),
            "response should be compact: {text}"
        );

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
    fn streaming_and_synchronous_paths_report_identically() {
        // The two entry points share one `run_script` behind a
        // ProgressSink, but that is an implementation detail — this
        // asserts the observable guarantee: whether or not a client
        // supplies a progressToken changes only the notifications, never
        // the report. When these were two 61%-duplicated functions,
        // nothing enforced that.
        use std::fs;
        let dir = std::env::temp_dir().join(format!("colab-mcp-equiv-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("main.go"), "package main\nimport \"old/pkg\"\n").unwrap();

        let script = "refactor \"r\" { match go::import \"old/pkg\" { replace \"new/pkg\" } }";
        let arguments = json!({
            "script": script,
            "paths": ["."],
            "cwd": dir.to_string_lossy(),
        });

        // Synchronous: no sink.
        let sync = run_script(
            &arguments,
            &registry(),
            &NoTools,
            RunMode::Preview,
            &mut NoProgress,
        )
        .expect("synchronous run");

        // Streaming: notifications go to a buffer we then ignore.
        let mut buffer: Vec<u8> = Vec::new();
        let token = json!("tok-1");
        let mut sink = Notifications {
            writer: &mut buffer,
            token: &token,
            last_emitted: 0,
        };
        let streamed = run_script(
            &arguments,
            &registry(),
            &NoTools,
            RunMode::Preview,
            &mut sink,
        )
        .expect("streaming run");

        // `elapsed_ms` is wall-clock and will differ; everything else
        // must match exactly.
        let strip = |mut v: Value| {
            v["summary"]
                .as_object_mut()
                .expect("summary")
                .remove("elapsed_ms");
            v
        };
        assert_eq!(
            strip(sync),
            strip(streamed),
            "the progressToken changed the report, not just the notifications"
        );

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
    fn apply_writes_nothing_while_a_rule_reports_a_conflict() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("colab-mcp-conflict-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let input = "package main\n\nfunc load() {}\n\nfunc parse() {}\n";
        fs::write(dir.join("main.go"), input).unwrap();
        let call = |allow: bool| {
            json!({
                "jsonrpc":"2.0","id":12,"method":"tools/call",
                "params":{"name":"colab.apply","arguments":{
                    "script":"refactor \"r\" { match go::symbol \"load\" { replace \"parse\" } }",
                    "paths":[dir.to_string_lossy()],
                    "allow_conflicts": allow,
                }}
            })
        };

        let resp = handle(&call(false), &registry()).unwrap();
        let parsed: Value =
            serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["applied"], false, "{parsed}");
        assert_eq!(parsed["conflicts"][0]["rule"], 0, "{parsed}");
        assert_eq!(fs::read_to_string(dir.join("main.go")).unwrap(), input);

        let resp = handle(&call(true), &registry()).unwrap();
        let parsed: Value =
            serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["applied"], true, "{parsed}");
        assert!(
            !fs::read_to_string(dir.join("main.go"))
                .unwrap()
                .contains("load")
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn workflow_tools_generate_the_script_and_preview_by_default() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("colab-mcp-workflow-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let go = "package main\n\nimport \"old/pkg\"\n";
        fs::write(dir.join("main.go"), go).unwrap();
        fs::write(dir.join("Cargo.toml"), "[dependencies]\nregex = \"1.9\"\n").unwrap();
        let call = |name: &str, args: Value| {
            let req = json!({"jsonrpc":"2.0","id":13,"method":"tools/call",
                "params":{"name": name, "arguments": args}});
            let resp = handle(&req, &registry()).unwrap();
            let text = resp["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string();
            serde_json::from_str::<Value>(&text).unwrap()
        };
        let paths = json!([dir.to_string_lossy()]);

        let preview = call(
            "colab.move_import",
            json!({"lang": "go", "from": "old/pkg", "to": "new/pkg", "paths": paths}),
        );
        assert_eq!(preview["applied"], false, "{preview}");
        assert_eq!(preview["rules"][0]["files"], 1, "{preview}");
        assert!(
            preview["script"]
                .as_str()
                .unwrap()
                .contains("match go::import \"old/pkg\" { replace \"new/pkg\" }")
        );
        assert_eq!(fs::read_to_string(dir.join("main.go")).unwrap(), go);

        let applied = call(
            "colab.move_import",
            json!({"lang": "go", "from": "old/pkg", "to": "new/pkg", "paths": paths, "write": true}),
        );
        assert_eq!(applied["applied"], true, "{applied}");
        assert!(
            fs::read_to_string(dir.join("main.go"))
                .unwrap()
                .contains("new/pkg")
        );

        let toml = dir.join("Cargo.toml");
        let set = call(
            "colab.set_config_key",
            json!({"key": "dependencies.regex", "value": "\"1.10\"", "paths": [toml.to_string_lossy()], "write": true}),
        );
        assert_eq!(set["applied"], true, "{set}");
        assert!(
            fs::read_to_string(&toml)
                .unwrap()
                .contains("regex = \"1.10\"")
        );

        let unknown = handle(&json!({"jsonrpc":"2.0","id":14,"method":"tools/call",
            "params":{"name":"colab.move_import","arguments":{"lang":"toml","from":"a","paths":paths}}}), &registry()).unwrap();
        assert_eq!(unknown["result"]["isError"], true, "{unknown}");
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

    /// Parse a buffer of newline-delimited JSON-RPC messages into a
    /// `Vec<Value>`, asserting each line is exactly one message.
    fn parse_messages(buf: &[u8]) -> Vec<Value> {
        let text = std::str::from_utf8(buf).unwrap();
        assert!(
            text.is_empty() || text.ends_with('\n'),
            "unterminated line: {text:?}"
        );
        text.lines()
            .map(|l| serde_json::from_str(l).expect("each line is one JSON message"))
            .collect()
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
                    .is_some_and(|s| s == "notifications/progress")
            })
            .collect();
        let responses: Vec<&Value> = messages.iter().filter(|m| m.get("id").is_some()).collect();

        assert!(
            progress.len() >= 2,
            "got {} progress messages",
            progress.len()
        );
        assert_eq!(responses.len(), 1, "got {responses:?}");
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
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
            "\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
        );
        let mut output: Vec<u8> = Vec::new();
        serve(input.as_bytes(), &mut output, registry()).unwrap();

        // One response per request, each on its own line, no headers.
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains("Content-Length"), "{text}");
        let messages = parse_messages(text.as_bytes());
        assert_eq!(messages.len(), 2, "{text}");
        assert_eq!(messages[0]["id"], 1);
        assert_eq!(messages[0]["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(messages[1]["id"], 2);
        assert!(messages[1]["result"]["tools"].is_array());
    }

    #[test]
    fn last_line_without_newline_is_still_read() {
        let input = "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/list\"}";
        let mut output: Vec<u8> = Vec::new();
        serve(input.as_bytes(), &mut output, registry()).unwrap();
        let messages = parse_messages(&output);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["id"], 3);
    }

    #[test]
    fn invalid_json_line_gets_parse_error_and_server_continues() {
        let input = "not json\n{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/list\"}\n";
        let mut output: Vec<u8> = Vec::new();
        serve(input.as_bytes(), &mut output, registry()).unwrap();
        let messages = parse_messages(&output);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["error"]["code"], PARSE_ERROR);
        assert!(messages[0]["id"].is_null());
        assert_eq!(messages[1]["id"], 4);
    }

    #[test]
    fn newlines_inside_strings_stay_on_one_line() {
        let mut output: Vec<u8> = Vec::new();
        write_message(&mut output, &json!({"text": "a\nb\r\nc"})).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text.matches('\n').count(), 1);
        assert!(text.ends_with('\n'));
        assert_eq!(parse_messages(text.as_bytes())[0]["text"], "a\nb\r\nc");
    }

    fn test_rule_call(arguments: Value) -> Value {
        let req = json!({
            "jsonrpc":"2.0","id":40,"method":"tools/call",
            "params":{"name":"colab.test_rule","arguments": arguments}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["result"]["isError"], false, "{resp}");
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn an_in_glob_is_relative_to_the_calls_cwd() {
        let dir = std::env::temp_dir().join(format!("colab-mcp-in-glob-{}", std::process::id()));
        for sub in ["api", "app"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
            std::fs::write(
                dir.join(sub).join("x.go"),
                "package x\n\nimport \"old/pkg\"\n",
            )
            .unwrap();
        }
        let script = r#"match go::import "old/pkg" in "api/**" { replace "new/pkg" }"#;
        let call = |tool: &str| {
            let req = json!({
                "jsonrpc":"2.0","id":41,"method":"tools/call",
                "params":{"name": tool, "arguments": {
                    "script": script, "paths": ["."], "cwd": dir.to_string_lossy()
                }}
            });
            let resp = handle(&req, &registry()).unwrap();
            assert_eq!(resp["result"]["isError"], false, "{resp}");
            let text = resp["result"]["content"][0]["text"].as_str().unwrap();
            serde_json::from_str::<Value>(text).unwrap()
        };
        let found = call("colab.find");
        assert_eq!(found["summary"]["matches"], 1, "{found}");
        assert!(found.to_string().contains("api/x.go"), "{found}");
        let preview = call("colab.preview");
        assert_eq!(preview["summary"]["changed"], 1, "{preview}");
        assert!(preview.to_string().contains("api/x.go"), "{preview}");
        assert!(!preview.to_string().contains("app/x.go"), "{preview}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    const TEST_RULE_SCRIPT: &str = "refactor \"x\" { match go::import \"a\" { replace \"b\" } }";
    const TEST_RULE_SOURCE: &str = "package main\n\nimport \"a\"\n";
    const TEST_RULE_OUTPUT: &str = "package main\n\nimport \"b\"\n";

    #[test]
    fn test_rule_transforms_a_snippet_in_memory() {
        let parsed = test_rule_call(json!({
            "script": TEST_RULE_SCRIPT,
            "source": TEST_RULE_SOURCE,
            "path": "main.go",
        }));
        assert_eq!(parsed["changed"], true);
        assert_eq!(parsed["output"], TEST_RULE_OUTPUT);
        assert_eq!(parsed["rules"][0]["files"], 1);
        assert!(parsed["diff"].as_str().unwrap().contains("+import \"b\""));
        assert!(parsed.get("warnings").is_none());
    }

    #[test]
    fn test_rule_compares_against_expected() {
        let ok = test_rule_call(json!({
            "script": TEST_RULE_SCRIPT,
            "source": TEST_RULE_SOURCE,
            "path": "main.go",
            "expected": TEST_RULE_OUTPUT,
        }));
        assert_eq!(ok["matches_expected"], true);
        assert!(ok.get("expected_diff").is_none());

        let bad = test_rule_call(json!({
            "script": TEST_RULE_SCRIPT,
            "source": TEST_RULE_SOURCE,
            "path": "main.go",
            "expected": "package main\n\nimport \"c\"\n",
        }));
        assert_eq!(bad["matches_expected"], false);
        assert!(
            bad["expected_diff"]
                .as_str()
                .unwrap()
                .contains("+import \"c\"")
        );
    }

    #[test]
    fn test_rule_warns_when_no_rule_applies_to_the_path() {
        let parsed = test_rule_call(json!({
            "script": TEST_RULE_SCRIPT,
            "source": TEST_RULE_SOURCE,
            "path": "main.rs",
        }));
        assert_eq!(parsed["changed"], false);
        assert_eq!(parsed["output"], TEST_RULE_SOURCE);
        let warnings = parsed["warnings"].to_string();
        assert!(warnings.contains("matched no files"), "{warnings}");
        assert!(
            warnings.contains("No rule applies to `main.rs`"),
            "{warnings}"
        );
    }

    #[test]
    fn test_rule_reports_script_errors_as_tool_errors() {
        let req = json!({
            "jsonrpc":"2.0","id":41,"method":"tools/call",
            "params":{"name":"colab.test_rule","arguments":{
                "script": "refactor \"x\" { match go::import \"a\" { replac \"b\" } }",
                "source": "", "path": "main.go"
            }}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["result"]["isError"], true);
    }

    #[test]
    fn test_rule_requires_source_and_path() {
        let req = json!({
            "jsonrpc":"2.0","id":42,"method":"tools/call",
            "params":{"name":"colab.test_rule","arguments":{"script": TEST_RULE_SCRIPT}}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["error"]["code"], INVALID_PARAMS);
    }

    fn call_ok(name: &str, arguments: Value) -> Value {
        let req = json!({
            "jsonrpc":"2.0","id":50,"method":"tools/call",
            "params":{"name": name, "arguments": arguments}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["result"]["isError"], false, "{resp}");
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn find_reports_pattern_matches_with_captures_relative_to_cwd() {
        let dir = std::env::temp_dir().join(format!("colab-mcp-find-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("main.go"),
            "package main\n\nfunc f() error {\n\treturn errors.Wrap(err, \"x\")\n}\n",
        )
        .unwrap();
        let parsed = call_ok(
            "colab.find",
            json!({
                "script": "refactor \"f\" { match go::pattern 'errors.Wrap($E, $M)' { delete } }",
                "paths": ["."],
                "cwd": dir.to_str().unwrap(),
                "detail": "detailed",
            }),
        );
        std::fs::remove_dir_all(&dir).ok();
        let m = &parsed["matches"][0];
        assert_eq!(m["path"], "main.go");
        assert_eq!((m["line"].as_u64(), m["col"].as_u64()), (Some(4), Some(9)));
        assert_eq!(m["captures"]["E"], "err");
        assert_eq!(parsed["summary"]["matches"], 1);
    }

    #[test]
    fn ast_shows_a_pattern_tree_and_its_context() {
        let parsed = call_ok("colab.ast", json!({"lang": "go", "pattern": "f($A)"}));
        assert!(
            parsed["tree"]
                .as_str()
                .unwrap()
                .starts_with("call_expression")
        );
        assert!(parsed["context"].as_str().unwrap().contains("func _()"));
        assert_eq!(parsed["prefilter"], "f");
    }

    #[test]
    fn ast_needs_exactly_one_input() {
        let req = json!({
            "jsonrpc":"2.0","id":51,"method":"tools/call",
            "params":{"name":"colab.ast","arguments":{"lang":"go"}}
        });
        let resp = handle(&req, &registry()).unwrap();
        assert_eq!(resp["error"]["code"], INVALID_PARAMS);
    }

    struct Extra;

    impl ToolProvider for Extra {
        fn tools(&self) -> Vec<Value> {
            vec![
                json!({"name": "colab.extra", "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": true}}),
            ]
        }
        fn call(&self, name: &str, args: &Value) -> Option<colab_core::Result<Value>> {
            (name == "colab.extra").then(|| match args.get("fail") {
                Some(_) => Err(colab_core::Error::Config("extra failed".into())),
                None => Ok(json!({"echo": args})),
            })
        }
    }

    #[test]
    fn a_host_provider_adds_tools_to_the_list_and_dispatch() {
        let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
        let resp = handle_with(&list, &registry(), &Extra).unwrap();
        let names: Vec<&str> = resp["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"colab.preview") && names.contains(&"colab.extra"));

        let call = |args: Value| {
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": "colab.extra", "arguments": args}})
        };
        let ok = handle_with(&call(json!({"x": 1})), &registry(), &Extra).unwrap();
        assert_eq!(ok["result"]["isError"], false);
        assert!(
            ok["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("\"x\":1")
        );
        let err = handle_with(&call(json!({"fail": true})), &registry(), &Extra).unwrap();
        assert_eq!(err["result"]["isError"], true);
        assert!(
            err["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("extra failed")
        );

        // Without the provider the tool does not exist.
        let plain = handle(&call(json!({})), &registry()).unwrap();
        assert_ne!(plain["result"]["isError"], false);
    }
}
