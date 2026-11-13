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
        let transport = Transport::spawn(program, args, root).map_err(|e| {
            Error::Config(format!(
                "could not start language server `{program}`: {e}. Is it installed and on PATH? (pass --lsp to choose another)"
            ))
        })?;
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
                            "workspaceFolders": true,
                            "configuration": true
                        },
                        "textDocument": {
                            "synchronization": {"didSave": false},
                            "rename": {"prepareSupport": true}
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

    pub fn name(&self) -> &str {
        &self.name
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
    pub fn wait_until_quiet(&mut self, settle: Duration) -> Result<()> {
        loop {
            let busy = !self.active_progress.is_empty() || self.quiescent == Some(false);
            let window = Instant::now() + settle;
            match self.transport.recv_until(window.min(self.deadline)) {
                Ok(message) => {
                    if message.get("method").is_some() {
                        self.serve(&message)?;
                    }
                }
                Err(Closed::TimedOut) if !busy => return Ok(()),
                Err(Closed::TimedOut) if Instant::now() >= self.deadline => {
                    return Err(Error::Config(format!(
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
            Err(Closed::TimedOut) => Err(Error::Config(format!(
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
                    .map(|t| t.to_string())
                    .unwrap_or_default();
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
            "workspace/applyEdit" => {
                // We never asked for one; refuse so nothing is applied behind
                // colab's report.
                json!({"jsonrpc": "2.0", "id": id, "result": {"applied": false, "failureReason": "colab applies edits itself"}})
            }
            _ => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not supported by colab"}})
            }
        };
        self.transport.send(&reply).map_err(|e| self.io_error(e))
    }

    pub fn server_error(&self, method: &str, e: &ResponseError) -> Error {
        Error::Config(format!(
            "`{}` rejected `{method}`: {} (code {})",
            self.name, e.message, e.code
        ))
    }

    fn exited(&self, during: &str) -> Error {
        Error::Config(format!(
            "language server `{}` exited while {during}",
            self.name
        ))
    }

    fn io_error(&self, e: std::io::Error) -> Error {
        Error::Config(format!("lost the connection to `{}`: {e}", self.name))
    }
}
