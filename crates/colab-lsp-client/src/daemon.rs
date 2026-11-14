//! A language server kept running between colab commands.
//!
//! Starting rust-analyzer or gopls and waiting for it to index costs
//! seconds, and an agent's colab commands come one after another: list the
//! code actions, run one, rename what it made. With `COLAB_KEEP_SERVER=1`
//! the first command starts `colab lsp-daemon`, which runs the server and
//! listens on a Unix socket; every later command for the same server and
//! project root connects to it instead of starting its own. The daemon
//! exits after [`IDLE`] without a client, or on `colab lsp-daemon --stop`.
//!
//! The daemon proxies the language server protocol, so the client code is
//! the same either way. It makes one server look fresh to each client:
//!
//! - **One handshake.** The first client's `initialize` reaches the server;
//!   later clients get its cached answer. A client's `shutdown` and `exit`
//!   end its own session, not the server.
//! - **No documents left open.** When a client goes, every document it
//!   opened is closed, so the server reads those files from disk again.
//! - **Nothing stale on disk.** colab writes files after a session ends.
//!   When the next client connects, the server is told which files under
//!   the root changed since (`workspace/didChangeWatchedFiles`).
//! - **Its own numbering.** Each client counts document versions and
//!   request ids from 1; the daemon renumbers them, so a late answer to a
//!   client that has gone is dropped, never given to the next one.
//! - **Indexing in progress is shown.** A client that connects while the
//!   server is still busy is told so (the open `$/progress` and the last
//!   `experimental/serverStatus`), so it waits as it would have.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use crate::edit::path_to_uri;
use crate::transport::{Closed, Transport, read_frame, write_frame};

/// How long a daemon waits for a client before shutting its server down.
pub const IDLE: Duration = Duration::from_secs(600);

/// The most files the daemon stats when a client connects. Past that it
/// tells the server nothing and relies on the server's own file watching.
const SCAN_LIMIT: usize = 50_000;

/// Whether commands should share a long-lived server: `COLAB_KEEP_SERVER=1`.
pub fn enabled() -> bool {
    std::env::var("COLAB_KEEP_SERVER").is_ok_and(|v| v == "1" || v == "true")
}

/// Where daemons keep their sockets: one directory per user.
pub fn socket_dir() -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "colab".into());
    std::env::temp_dir().join(format!("colab-lsp-{user}"))
}

/// The socket for `program args…` serving `root`.
pub fn socket_for(program: &str, args: &[String], root: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    program.hash(&mut hasher);
    args.hash(&mut hasher);
    root.hash(&mut hasher);
    socket_dir().join(format!("{:016x}.sock", hasher.finish()))
}

/// A connection to the daemon for `program args…` in `root`, starting one
/// (`<exe> lsp-daemon …`) when none is listening. `exe` is the colab
/// binary; `COLAB_EXE` overrides it.
pub fn connect_or_start(
    exe: &Path,
    program: &str,
    args: &[String],
    root: &Path,
    timeout: Duration,
) -> io::Result<UnixStream> {
    let socket = socket_for(program, args, root);
    if let Ok(stream) = UnixStream::connect(&socket) {
        return Ok(stream);
    }
    // A socket nobody answers on is left over from a daemon that died.
    let _ = std::fs::remove_file(&socket);
    std::fs::create_dir_all(socket_dir())?;
    let exe = std::env::var_os("COLAB_EXE").map_or_else(|| exe.to_path_buf(), PathBuf::from);
    std::process::Command::new(exe)
        .arg("lsp-daemon")
        .arg("--socket")
        .arg(&socket)
        .arg("--root")
        .arg(root)
        .arg("--")
        .arg(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + timeout.min(Duration::from_secs(10));
    loop {
        match UnixStream::connect(&socket) {
            Ok(stream) => return Ok(stream),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => thread::sleep(Duration::from_millis(25)),
        }
    }
}

/// Ask the daemon on `socket` to stop.
pub fn stop(socket: &Path) -> io::Result<()> {
    let mut stream = UnixStream::connect(socket)?;
    write_frame(
        &mut stream,
        &json!({"jsonrpc": "2.0", "method": "colab/stop"}),
    )
}

/// Ask every daemon of this user to stop, or with `root` only those
/// serving it. Returns how many answered.
pub fn stop_all(root: Option<&Path>) -> usize {
    let Ok(entries) = std::fs::read_dir(socket_dir()) else {
        return 0;
    };
    let mut stopped = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "sock") {
            continue;
        }
        if let Some(root) = root {
            let served = std::fs::read_to_string(path.with_extension("root")).unwrap_or_default();
            if Path::new(served.trim()) != root {
                continue;
            }
        }
        match UnixStream::connect(&path) {
            Ok(mut stream) => {
                if write_frame(
                    &mut stream,
                    &json!({"jsonrpc": "2.0", "method": "colab/stop"}),
                )
                .is_ok()
                {
                    stopped += 1;
                }
            }
            Err(_) => {
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_file(path.with_extension("root"));
            }
        }
    }
    stopped
}

/// What the daemon remembers about its server between clients.
struct Server {
    transport: Transport,
    root: PathBuf,
    /// The answer to the first `initialize`, given to every later client.
    initialized: Option<Value>,
    /// `initialized` has been sent once; it is never sent again.
    announced: bool,
    /// The version the server last saw for each open document.
    versions: HashMap<String, i64>,
    /// `$/progress` begin notifications not yet ended, by token.
    progress: HashMap<String, Value>,
    /// The latest `experimental/serverStatus`.
    status: Option<Value>,
    /// When the last client went, for the files-changed scan.
    last_client: Option<SystemTime>,
    session: u64,
}

/// Run the daemon: start `program args…` in `root`, listen on `socket`,
/// serve clients one at a time, and exit after `idle` without one.
pub fn serve(
    program: &str,
    args: &[String],
    root: &Path,
    socket: &Path,
    idle: Duration,
) -> io::Result<()> {
    let listener = UnixListener::bind(socket)?;
    listener.set_nonblocking(true)?;
    // Beside the socket, for `colab lsp-daemon --stop --root`.
    std::fs::write(
        socket.with_extension("root"),
        root.to_string_lossy().as_bytes(),
    )?;
    let mut server = Server {
        transport: Transport::spawn(program, args, root)?,
        root: root.to_path_buf(),
        initialized: None,
        announced: false,
        versions: HashMap::new(),
        progress: HashMap::new(),
        status: None,
        last_client: None,
        session: 0,
    };
    let mut idle_since = Instant::now();
    let result = loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                match server.client(stream) {
                    Ok(true) => break Ok(()),
                    Ok(false) => {}
                    Err(e) => break Err(e),
                }
                idle_since = Instant::now();
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if idle_since.elapsed() >= idle {
                    break Ok(());
                }
                // Between clients: keep up with the server.
                if let Ok(message) = server
                    .transport
                    .recv_until(Instant::now() + Duration::from_millis(50))
                {
                    server.unattended(&message)?;
                }
            }
            Err(e) => break Err(e),
        }
    };
    let _ = std::fs::remove_file(socket);
    let _ = std::fs::remove_file(socket.with_extension("root"));
    let _ = server
        .transport
        .send(&json!({"jsonrpc": "2.0", "id": "colab-shutdown", "method": "shutdown"}));
    let _ = server
        .transport
        .recv_until(Instant::now() + Duration::from_secs(2));
    let _ = server
        .transport
        .send(&json!({"jsonrpc": "2.0", "method": "exit"}));
    result
}

impl Server {
    /// Track what the server says, whoever it is said to.
    fn observe(&mut self, message: &Value) {
        match message.get("method").and_then(Value::as_str) {
            Some("$/progress") => {
                let token = message
                    .pointer("/params/token")
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                match message
                    .pointer("/params/value/kind")
                    .and_then(Value::as_str)
                {
                    Some("begin") => {
                        self.progress.insert(token, message.clone());
                    }
                    Some("end") => {
                        self.progress.remove(&token);
                    }
                    _ => {}
                }
            }
            Some("experimental/serverStatus") => self.status = Some(message.clone()),
            _ => {}
        }
    }

    /// A message from the server with no client to give it to: answer its
    /// requests as a client that offers nothing would.
    fn unattended(&mut self, message: &Value) -> io::Result<()> {
        self.observe(message);
        let (Some(id), Some(method)) = (
            message.get("id"),
            message.get("method").and_then(Value::as_str),
        ) else {
            return Ok(());
        };
        let result = match method {
            "workspace/configuration" => {
                let n = message
                    .pointer("/params/items")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                json!(vec![Value::Null; n])
            }
            "workspace/applyEdit" => json!({"applied": false}),
            _ => Value::Null,
        };
        self.transport
            .send(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    /// Files under the root modified since the last client went, as
    /// `didChangeWatchedFiles` changes.
    fn changed_since(&self, since: SystemTime) -> Vec<Value> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        let mut seen = 0;
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                seen += 1;
                if seen > SCAN_LIMIT {
                    return Vec::new();
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                let path = entry.path();
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    stack.push(path);
                } else if meta.modified().is_ok_and(|m| m >= since) {
                    out.push(json!({"uri": path_to_uri(&path), "type": 2}));
                }
            }
        }
        out
    }

    /// Serve one client to the end. `Ok(true)` when it asked the daemon
    /// to stop.
    fn client(&mut self, stream: UnixStream) -> io::Result<bool> {
        self.session += 1;
        let session = self.session;
        let mut writer = stream.try_clone()?;
        let (tx, from_client) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            while let Some(message) = read_frame(&mut reader) {
                if tx.send(message).is_err() {
                    break;
                }
            }
        });
        // Request ids this client sent, as the server sees them.
        let mut pending: HashMap<String, Value> = HashMap::new();
        let mut opened: HashSet<String> = HashSet::new();
        let mut init_id: Option<String> = None;
        if let Some(since) = self.last_client.take() {
            let changes = self.changed_since(since);
            if !changes.is_empty() {
                self.transport.send(&json!({
                    "jsonrpc": "2.0",
                    "method": "workspace/didChangeWatchedFiles",
                    "params": {"changes": changes}
                }))?;
            }
        }
        let stop = loop {
            // From the client.
            match from_client.recv_timeout(Duration::from_millis(5)) {
                Ok(mut message) => {
                    let method = message
                        .get("method")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let id = message.get("id").cloned();
                    match (method.as_deref(), id) {
                        (Some("colab/stop"), _) => break true,
                        (Some("initialize"), Some(id)) => match &self.initialized {
                            Some(result) => {
                                write_frame(
                                    &mut writer,
                                    &json!({"jsonrpc": "2.0", "id": id, "result": result}),
                                )?;
                                // Show indexing that is still going on.
                                for begin in self.progress.values() {
                                    write_frame(&mut writer, begin)?;
                                }
                                if let Some(status) = &self.status {
                                    write_frame(&mut writer, status)?;
                                }
                            }
                            None => {
                                let server_id = format!("s{session}-{id}");
                                init_id = Some(server_id.clone());
                                pending.insert(server_id.clone(), id);
                                message["id"] = json!(server_id);
                                self.transport.send(&message)?;
                            }
                        },
                        (Some("initialized"), None) => {
                            if !self.announced {
                                self.announced = true;
                                self.transport.send(&message)?;
                            }
                        }
                        (Some("shutdown"), Some(id)) => {
                            write_frame(
                                &mut writer,
                                &json!({"jsonrpc": "2.0", "id": id, "result": null}),
                            )?;
                        }
                        (Some("exit"), None) => break false,
                        (
                            Some(
                                m @ ("textDocument/didOpen"
                                | "textDocument/didChange"
                                | "textDocument/didClose"),
                            ),
                            None,
                        ) => {
                            let uri = message
                                .pointer("/params/textDocument/uri")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            if m == "textDocument/didClose" {
                                opened.remove(&uri);
                                self.versions.remove(&uri);
                            } else {
                                let version = self.versions.get(&uri).map_or(1, |v| v + 1);
                                self.versions.insert(uri.clone(), version);
                                opened.insert(uri);
                                message["params"]["textDocument"]["version"] = json!(version);
                            }
                            self.transport.send(&message)?;
                        }
                        // A request: renumber it, so its answer finds this client.
                        (Some(_), Some(id)) => {
                            let server_id = format!("s{session}-{id}");
                            pending.insert(server_id.clone(), id);
                            message["id"] = json!(server_id);
                            self.transport.send(&message)?;
                        }
                        // A notification, or an answer to the server's request.
                        _ => self.transport.send(&message)?,
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break false,
            }
            // From the server.
            loop {
                match self.transport.recv_until(Instant::now()) {
                    Ok(mut message) => {
                        self.observe(&message);
                        let is_response = message.get("method").is_none();
                        if is_response {
                            let key = message
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let Some(client_id) = pending.remove(&key) else {
                                continue; // an answer for a client that has gone
                            };
                            if init_id.as_deref() == Some(key.as_str()) {
                                self.initialized = message.get("result").cloned();
                            }
                            message["id"] = client_id;
                        }
                        if write_frame(&mut writer, &message).is_err() {
                            break;
                        }
                    }
                    Err(Closed::TimedOut) => break,
                    Err(Closed::Exited) => {
                        return Err(io::Error::other("the language server exited"));
                    }
                }
            }
        };
        // The next client must find the server as if it were fresh.
        for uri in opened {
            self.versions.remove(&uri);
            self.transport.send(&json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didClose",
                "params": {"textDocument": {"uri": uri}}
            }))?;
        }
        self.last_client = Some(SystemTime::now());
        Ok(stop)
    }
}
