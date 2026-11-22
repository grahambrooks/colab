//! One conversation with a language server: handshake, requests (while
//! answering the server's own requests), and "wait until indexing is
//! done".

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use colab_core::{Error, Result};
use serde_json::{Value, json};

use crate::edit::{Encoding, path_to_uri};
use crate::transport::{Closed, Transport};

/// JSON-RPC error codes that mean "try again shortly": the document or
/// project changed under the request, or the server is not ready.
const RETRYABLE: [i64; 3] = [-32801, -32802, -32803];

/// A JSON-RPC error the server answered with.
#[derive(Debug, Clone)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
}

/// The silence [`Session::wait_until_quiet`] waits for once the server has
/// reported itself quiescent.
pub const QUIESCENT_SETTLE: Duration = Duration::from_millis(50);

pub struct Session {
    transport: Transport,
    name: String,
    next_id: u64,
    pub encoding: Encoding,
    pub capabilities: Value,
    /// `$/progress` tokens that have begun and not ended.
    active_progress: HashSet<String>,
    /// rust-analyzer's `experimental/serverStatus.quiescent`, once seen.
    quiescent: Option<bool>,
    deadline: Instant,
    root_uri: String,
    /// While `Some`, `workspace/applyEdit` requests are accepted and their
    /// edits collected here instead of being refused — the result of a
    /// code action that runs a command (see [`Session::capture_edits`]).
    captured: Option<Vec<Value>>,
    /// Documents opened so far, with their version.
    versions: std::collections::HashMap<String, i64>,
    /// The latest `textDocument/publishDiagnostics` for each URI.
    published: std::collections::HashMap<String, Vec<Value>>,
}

impl Session {
    /// Spawn the server in `root` and complete `initialize` / `initialized`.
    ///
    /// `file_operations` says whether the caller can make create / rename /
    /// delete operations; it decides what the client advertises.
    pub fn start(
        program: &str,
        args: &[String],
        root: &Path,
        timeout: Duration,
        file_operations: bool,
    ) -> Result<Self> {
        let mut span = colab_core::telemetry::client_span("lsp start");
        span.set("colab.lsp.server", server_name(program));
        let session = Self::start_untraced(program, args, root, timeout, file_operations);
        if let Err(e) = &session {
            span.fail(e.kind());
        }
        session
    }

    fn start_untraced(
        program: &str,
        args: &[String],
        root: &Path,
        timeout: Duration,
        file_operations: bool,
    ) -> Result<Self> {
        let transport = Self::transport(program, args, root, timeout)?;
        let mut session = Self {
            transport,
            name: program.to_string(),
            next_id: 1,
            encoding: Encoding::Utf16,
            capabilities: Value::Null,
            active_progress: HashSet::new(),
            quiescent: None,
            deadline: Instant::now() + timeout,
            root_uri: path_to_uri(root),
            captured: None,
            versions: Default::default(),
            published: Default::default(),
        };
        let root_uri = session.root_uri.clone();
        let resource_operations = if file_operations {
            json!(["create", "rename", "delete"])
        } else {
            json!([])
        };
        let result = session
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "rootPath": root.display().to_string(),
                    "workspaceFolders": [{"uri": root_uri, "name": "root"}],
                    "clientInfo": {"name": "colab", "version": env!("CARGO_PKG_VERSION")},
                    "capabilities": {
                        "general": {"positionEncodings": ["utf-8", "utf-16"]},
                        "window": {"workDoneProgress": true},
                        "workspace": {
                            "workspaceEdit": {"documentChanges": true, "resourceOperations": resource_operations},
                            "fileOperations": {"willRename": file_operations},
                            "applyEdit": true,
                            "executeCommand": {},
                            "workspaceFolders": true,
                            "configuration": true
                        },
                        "textDocument": {
                            "synchronization": {"didSave": false},
                            "rename": {"prepareSupport": true},
                            "codeAction": {
                                "codeActionLiteralSupport": {"codeActionKind": {"valueSet": [
                                    "", "quickfix", "refactor", "refactor.extract",
                                    "refactor.inline", "refactor.rewrite", "refactor.move",
                                    "source", "source.organizeImports", "source.fixAll"
                                ]}},
                                "resolveSupport": {"properties": ["edit", "command"]},
                                "dataSupport": true,
                                "isPreferredSupport": true,
                                "disabledSupport": true
                            },
                            "references": {},
                            "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
                            "formatting": {}
                        },
                        "experimental": {"serverStatusNotification": true}
                    }
                }),
            )?
            .map_err(|e| session.server_error("initialize", &e))?;
        session.encoding = Encoding::parse(
            result
                .pointer("/capabilities/positionEncoding")
                .and_then(Value::as_str),
        );
        session.capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);
        session.notify("initialized", json!({}))?;
        Ok(session)
    }

    /// The server's own process, or, when servers are kept (the default), a
    /// connection to the `colab lsp-daemon` that keeps one running.
    fn transport(
        program: &str,
        args: &[String],
        root: &Path,
        timeout: Duration,
    ) -> Result<Transport> {
        // A kept server, unless that fails: then this command gets its own,
        // as it would with `COLAB_KEEP_SERVER=0`. Off Unix there are none,
        // and the start-up timeout is only the kept server's.
        #[cfg(not(unix))]
        let _ = timeout;
        #[cfg(unix)]
        if crate::daemon::enabled() {
            let kept = std::env::current_exe().and_then(|exe| {
                crate::daemon::connect_or_start(&exe, program, args, root, timeout)
                    .and_then(Transport::over)
            });
            match kept {
                Ok(transport) => return Ok(transport),
                Err(e) => log::warn!(
                    "could not reach a kept `{program}` ({e}); starting one for this command"
                ),
            }
        }
        Transport::spawn(program, args, root).map_err(|e| {
            Error::Server(format!(
                "could not start language server `{program}`: {e}. Is it installed and on PATH? (pass --lsp to choose another)"
            ))
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Tell the server about `path` with contents `text`: `didOpen` the
    /// first time, a full-text `didChange` after that.
    pub fn open(&mut self, path: &Path, text: &str) -> Result<()> {
        let uri = path_to_uri(path);
        match self.versions.get_mut(&uri) {
            Some(version) => {
                *version += 1;
                let version = *version;
                self.notify(
                    "textDocument/didChange",
                    json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
                )
            }
            None => {
                self.versions.insert(uri.clone(), 1);
                self.notify(
                    "textDocument/didOpen",
                    json!({"textDocument": {"uri": uri, "languageId": crate::language_id(path), "version": 1, "text": text}}),
                )
            }
        }
    }

    /// The diagnostics the server last pushed for `path`, if it has.
    pub fn published(&self, path: &Path) -> Option<&[Value]> {
        self.published.get(&path_to_uri(path)).map(Vec::as_slice)
    }

    /// Forget what was pushed for `path`, so the next look waits for a
    /// fresh publish rather than reading a stale one.
    pub fn forget_published(&mut self, path: &Path) {
        self.published.remove(&path_to_uri(path));
    }

    /// Start (or stop and collect) accepting `workspace/applyEdit`. A code
    /// action that is a *command* makes its change by sending the client
    /// an edit to apply; colab takes that edit as the action's result and
    /// writes it only through its own report.
    pub fn capture_edits(&mut self, on: bool) -> Vec<Value> {
        if on {
            self.captured = Some(Vec::new());
            Vec::new()
        } else {
            self.captured.take().unwrap_or_default()
        }
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.transport
            .send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .map_err(|e| self.io_error(e))
    }

    /// Send a request and wait for its response, serving the server's own
    /// requests and notifications meanwhile. The outer `Result` is a
    /// transport failure or timeout; the inner one is the server's answer.
    pub fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<std::result::Result<Value, ResponseError>> {
        let mut span = self.span(method);
        let answer = self.request_untraced(method, params);
        match &answer {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => span.fail(format!("error {}", e.code)),
            Err(e) => span.fail(e.kind()),
        }
        answer
    }

    /// A client span for a request of `method` to this server.
    fn span(&self, method: &str) -> colab_core::telemetry::Span {
        let mut span = colab_core::telemetry::client_span(format!("lsp {method}"));
        if span.is_recording() {
            span.set("rpc.system", "jsonrpc");
            span.set("rpc.method", method);
            span.set("colab.lsp.server", server_name(&self.name));
        }
        span
    }

    fn request_untraced(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<std::result::Result<Value, ResponseError>> {
        let id = self.next_id;
        self.next_id += 1;
        self.transport
            .send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .map_err(|e| self.io_error(e))?;
        loop {
            let message = self.recv(method)?;
            if message.get("method").is_some() {
                self.serve(&message)?;
                continue;
            }
            if message.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Ok(Err(ResponseError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                }));
            }
            return Ok(Ok(message.get("result").cloned().unwrap_or(Value::Null)));
        }
    }

    /// Several requests of one `method`, pipelined: all are sent before
    /// any answer is read, so a batch costs about one round trip. Answers
    /// come back in `params` order; one the server was not ready for is
    /// asked again on its own ([`request_retrying`](Self::request_retrying)).
    pub fn request_all(
        &mut self,
        method: &str,
        params: Vec<Value>,
    ) -> Result<Vec<std::result::Result<Value, ResponseError>>> {
        let mut span = self.span(method);
        span.set("colab.lsp.requests", params.len());
        let answers = self.request_all_untraced(method, params);
        if let Err(e) = &answers {
            span.fail(e.kind());
        }
        answers
    }

    fn request_all_untraced(
        &mut self,
        method: &str,
        params: Vec<Value>,
    ) -> Result<Vec<std::result::Result<Value, ResponseError>>> {
        let first = self.next_id;
        self.next_id += params.len() as u64;
        for (i, p) in params.iter().enumerate() {
            let id = first + i as u64;
            self.transport
                .send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": p}))
                .map_err(|e| self.io_error(e))?;
        }
        let mut answers: Vec<Option<std::result::Result<Value, ResponseError>>> =
            vec![None; params.len()];
        let mut left = params.len();
        while left > 0 {
            let message = self.recv(method)?;
            if message.get("method").is_some() {
                self.serve(&message)?;
                continue;
            }
            let Some(slot) = message
                .get("id")
                .and_then(Value::as_u64)
                .and_then(|id| id.checked_sub(first))
                .and_then(|i| usize::try_from(i).ok())
                .filter(|&i| i < answers.len() && answers[i].is_none())
            else {
                continue;
            };
            answers[slot] = Some(match message.get("error") {
                Some(error) => Err(ResponseError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                }),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            });
            left -= 1;
        }
        let mut out = Vec::with_capacity(answers.len());
        for (answer, p) in answers.into_iter().zip(params) {
            out.push(match answer.expect("every slot was answered") {
                Err(e) if RETRYABLE.contains(&e.code) => self.request_retrying(method, p)?,
                other => other,
            });
        }
        Ok(out)
    }

    /// [`request`](Self::request), retried while the server answers with a
    /// "not ready / content modified" error.
    pub fn request_retrying(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<std::result::Result<Value, ResponseError>> {
        let mut backoff = Duration::from_millis(200);
        loop {
            match self.request(method, params.clone())? {
                Err(e)
                    if RETRYABLE.contains(&e.code) && Instant::now() + backoff < self.deadline =>
                {
                    self.idle(backoff)?;
                    backoff = (backoff * 2).min(Duration::from_secs(2));
                }
                other => return Ok(other),
            }
        }
    }

    /// Wait until the server has no work in progress and has been silent
    /// for `settle`. A rename sent while a server is still indexing can
    /// come back *partial* — renaming only what it has seen — so this is a
    /// correctness wait, not a courtesy.
    ///
    /// A server that says when it is done — rust-analyzer's
    /// `experimental/serverStatus` — needs no long silence once it has
    /// said `quiescent: true` with no progress open: the window shrinks to
    /// [`QUIESCENT_SETTLE`], long enough to see it turn busy again after a
    /// `didOpen`. (A kept server's daemon replays its last status, so a warm
    /// command does not pay the full `settle` each time.) Servers that
    /// report only `$/progress` keep the full window.
    pub fn wait_until_quiet(&mut self, settle: Duration) -> Result<()> {
        loop {
            let busy = !self.active_progress.is_empty() || self.quiescent == Some(false);
            let said_done = self.quiescent == Some(true) && self.active_progress.is_empty();
            let window = Instant::now()
                + if said_done {
                    settle.min(QUIESCENT_SETTLE)
                } else {
                    settle
                };
            match self.transport.recv_until(window.min(self.deadline)) {
                Ok(message) => {
                    if message.get("method").is_some() {
                        self.serve(&message)?;
                    }
                }
                Err(Closed::TimedOut) if !busy => return Ok(()),
                Err(Closed::TimedOut) if Instant::now() >= self.deadline => {
                    return Err(Error::Server(format!(
                        "`{}` was still indexing when the timeout expired; a rename now could be incomplete. Raise --timeout.",
                        self.name
                    )));
                }
                Err(Closed::TimedOut) => {}
                Err(Closed::Exited) => return Err(self.exited("waiting for indexing")),
            }
        }
    }

    /// Serve messages for `duration` without waiting on anything.
    fn idle(&mut self, duration: Duration) -> Result<()> {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            match self.transport.recv_until(until) {
                Ok(message) if message.get("method").is_some() => self.serve(&message)?,
                Ok(_) | Err(Closed::TimedOut) => {}
                Err(Closed::Exited) => return Err(self.exited("retrying")),
            }
        }
        Ok(())
    }

    /// Polite shutdown. Errors are ignored: the edits are already in hand.
    pub fn shutdown(mut self) {
        self.deadline = Instant::now() + Duration::from_secs(2);
        if self.request("shutdown", Value::Null).is_ok() {
            let _ = self.notify("exit", Value::Null);
        }
    }

    fn recv(&mut self, waiting_for: &str) -> Result<Value> {
        match self.transport.recv_until(self.deadline) {
            Ok(message) => Ok(message),
            Err(Closed::TimedOut) => Err(Error::Server(format!(
                "`{}` did not answer `{waiting_for}` before the timeout; raise --timeout",
                self.name
            ))),
            Err(Closed::Exited) => Err(self.exited(waiting_for)),
        }
    }

    /// Track progress, and answer any request the server sends us.
    fn serve(&mut self, message: &Value) -> Result<()> {
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "$/progress" => {
                let token = params
                    .get("token")
                    .map_or_default(std::string::ToString::to_string);
                match params.pointer("/value/kind").and_then(Value::as_str) {
                    Some("begin") => {
                        self.active_progress.insert(token);
                    }
                    Some("end") => {
                        self.active_progress.remove(&token);
                    }
                    _ => {}
                }
            }
            "experimental/serverStatus" => {
                self.quiescent = params.get("quiescent").and_then(Value::as_bool);
            }
            "textDocument/publishDiagnostics" => {
                if let Some(uri) = params.get("uri").and_then(Value::as_str) {
                    let list = params
                        .get("diagnostics")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    self.published.insert(uri.to_string(), list);
                }
            }
            _ => {}
        }
        let Some(id) = message.get("id").cloned() else {
            return Ok(()); // a notification
        };
        let reply = match method {
            "workspace/configuration" => {
                let items = params
                    .get("items")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                json!({"jsonrpc": "2.0", "id": id, "result": vec![Value::Null; items]})
            }
            "workspace/workspaceFolders" => {
                json!({"jsonrpc": "2.0", "id": id, "result": [{"uri": self.root_uri, "name": "root"}]})
            }
            "window/workDoneProgress/create"
            | "client/registerCapability"
            | "client/unregisterCapability"
            | "window/showMessageRequest"
            | "window/showDocument"
            | "workspace/semanticTokens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/codeLens/refresh"
            | "workspace/diagnostic/refresh" => {
                json!({"jsonrpc": "2.0", "id": id, "result": Value::Null})
            }
            "workspace/applyEdit" => match self.captured.as_mut() {
                Some(captured) => {
                    captured.push(params.get("edit").cloned().unwrap_or(Value::Null));
                    // "Applied" to colab's change set, which is reported
                    // and written (or not) by colab itself.
                    json!({"jsonrpc": "2.0", "id": id, "result": {"applied": true}})
                }
                // Not running a command: refuse so nothing is applied
                // behind colab's report.
                None => {
                    json!({"jsonrpc": "2.0", "id": id, "result": {"applied": false, "failureReason": "colab applies edits itself"}})
                }
            },
            _ => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not supported by colab"}})
            }
        };
        self.transport.send(&reply).map_err(|e| self.io_error(e))
    }

    pub fn server_error(&self, method: &str, e: &ResponseError) -> Error {
        Error::Server(format!(
            "`{}` rejected `{method}`: {} (code {})",
            self.name, e.message, e.code
        ))
    }

    fn exited(&self, during: &str) -> Error {
        Error::Server(format!(
            "language server `{}` exited while {during}",
            self.name
        ))
    }

    fn io_error(&self, e: std::io::Error) -> Error {
        Error::Server(format!("lost the connection to `{}`: {e}", self.name))
    }
}

/// A server's program name for a span: the file name, never the path it
/// was started from.
fn server_name(program: &str) -> String {
    Path::new(program)
        .file_name()
        .map_or_else(|| program.to_string(), |n| n.to_string_lossy().into_owned())
}
