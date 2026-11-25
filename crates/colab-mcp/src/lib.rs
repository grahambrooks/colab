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
use colab_dsl::compile;
use serde_json::{Map, Value, json};

mod args;
mod progress;
pub mod resources;
mod rules;
pub use rules::script_from_rules_json;
mod schema;
mod scripts;
mod tools;
mod workflows;

use args::*;
use progress::*;
use schema::*;
use scripts::*;

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
Always pass `cwd`. Give `rules` as JSON (no DSL to learn):\n\
  {\"rules\": [{\"lang\": \"rust\", \"match\": \"serde_yaml::from_str($S)\", \"replace\": \"serde_saphyr::from_str($S)\"},\n\
             {\"lang\": \"toml\", \"module\": \"key\", \"match\": \"package.edition\", \"set\": \"\\\"2024\\\"\"}]}\n\
`match` is code with `$X` (one node) and `$$$X` (a sequence); the action is one of replace, \
delete: true, set, insert, …; `where` narrows: [{\"not_inside\": \"mod tests { $$$B }\"}, \
{\"var\": \"$X\", \"matches\": \"^cfg\"}]; `in` takes a glob. The reply carries the equivalent \
`script`, which you may also send instead of `rules`. A rule reported with `files: 0` matched \
nothing: fix the rule, do not fall back to sed.\n\
\n\
Also: colab.rename_symbol / colab.rename (scope-aware, via the language server), \
colab.edit_symbol (replace one declaration by name), colab.outline / colab.section / \
colab.edit_markdown (Markdown by heading), colab.set_config_key, colab.move_import. \
colab.schema / colab.list_rules list each language's modules; colab.lint_script checks a script.\n\
\n\
Already made the change once by hand? colab.generalize {lang, before, after, paths, cwd} turns that \
edit into a rule (carried and changed arguments become $METAVARS) and lists its other matches; \
pass the returned `rule` to colab.apply as `rules: [rule]`.";

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

    /// The project's own expression macros for a tool call run in `cwd`,
    /// whose contents patterns read as they read `assert!`'s: the binary
    /// reads them from the `colab.toml` at or above `cwd` (`[patterns]
    /// macros`). None by default.
    fn macros(&self, _cwd: Option<&Path>) -> Vec<String> {
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
    let response =
        call_run(arguments, extra).enter(|| handle_call_in_run(id, params, backends, extra));
    trace_outcome(&mut span, &response);
    response
}

/// A run for one tool call. The tools join relative `paths` onto `cwd`,
/// so the walk yields absolute paths; the call's `cwd` is the run's base,
/// so a rule's `in "<glob>"` reads relative to it, as it does from the
/// CLI's working directory.
/// The project's own expression macros (`extra.macros`) are the run's too.
fn call_run(
    arguments: Option<&Value>,
    extra: &dyn ToolProvider,
) -> std::sync::Arc<colab_core::run::Run> {
    let run = colab_core::run::Run::new();
    let cwd = arguments
        .and_then(|a| a.get("cwd"))
        .and_then(Value::as_str)
        .map(PathBuf::from);
    run.set_macros(extra.macros(cwd.as_deref()));
    run.set_base(cwd);
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
            // An error in an `include`d file is in that file, not the script sent.
            if let Some(path) = &detail.path {
                payload.insert("path".into(), json!(path.display().to_string()));
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
    let outcome = call_run(Some(&arguments), extra)
        .enter(|| run_script(&arguments, backends, extra, mode, &mut sink));
    let response = tool_response(id, outcome);
    trace_outcome(&mut span, &response);
    Ok(response)
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
            let script = crate::rules::script_arg(args)?;
            let cwd = arg_opt_path(args, "cwd")?;
            let refactoring = compile_script(&script.text, cwd.as_deref(), backends)
                .map_err(ToolError::from_error)?;
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
            let report = refactoring.report();
            let warnings = report.warnings();
            if !warnings.is_empty() {
                doc["warnings"] = json!(warnings);
            }
            if script.generated {
                doc["script"] = json!(script.text);
            }
            Ok(doc)
        }
        "colab.test_rule" => test_rule(args, backends),
        "colab.find" => find(args, backends),
        "colab.ast" => ast(args, backends),
        "colab.generalize" => rules::generalize(args, backends),
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
mod tests;
