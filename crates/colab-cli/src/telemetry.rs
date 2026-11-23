//! OTLP export of colab's traces ([`colab_core::telemetry`]), configured
//! by the standard OpenTelemetry environment variables and, beneath them,
//! by the `[telemetry]` table of the user's config file, so any collector
//! can receive them — a local one such as habit, an OpenTelemetry
//! Collector, Jaeger.
//!
//! - **Off unless an endpoint is set**: `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`
//!   (used as given), `OTEL_EXPORTER_OTLP_ENDPOINT` (`/v1/traces`
//!   appended), or the file's `endpoint` (a base URL, likewise); the
//!   file's `enabled = true` alone sends to `http://127.0.0.1:4318`.
//!   `OTEL_SDK_DISABLED=true`, `OTEL_TRACES_EXPORTER=none` or the file's
//!   `enabled = false` turn it off again, whatever else is set.
//! - **The file** is `$XDG_CONFIG_HOME/colab/config.toml`, else
//!   `~/.config/colab/config.toml`: a user's setting, never a repo's
//!   (`colab.toml` is not read for it). It exists because the processes an
//!   agent starts (Claude Code's Bash tool, its MCP servers) do not inherit
//!   the agent's `OTEL_*` variables. A missing file is no telemetry config;
//!   an unreadable or malformed one is ignored, with a `debug` line.
//!   Environment variables win over it key by key.
//! - **Long-running commands** (`mcp`, `server`, `lsp-daemon`) export from
//!   a background thread, so a request's response never waits on the
//!   collector. A one-shot command exports at its end, within the timeout.
//! - **OTLP/HTTP with JSON bodies**, over plain `http://` (colab ships no
//!   TLS stack; an `https://` endpoint turns export off). A gRPC endpoint
//!   on port 4317 — `OTEL_EXPORTER_OTLP_PROTOCOL=grpc`, as Claude Code's
//!   settings often set, and colab inherits from it — is sent to the same
//!   host's OTLP/HTTP port, 4318.
//! - `OTEL_SERVICE_NAME` (default `colab`), `OTEL_RESOURCE_ATTRIBUTES` and
//!   `OTEL_EXPORTER_OTLP_(TRACES_)HEADERS` are honoured;
//!   `OTEL_EXPORTER_OTLP_(TRACES_)TIMEOUT` (milliseconds) bounds each export,
//!   500 ms by default, so a command never waits long on telemetry.
//! - **Never fails a command.** An export that cannot be made is dropped,
//!   with a `debug` log line.

use std::io::{Read, Write as _};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use colab_core::telemetry::{self, Kind, SpanData, Value};
use log::debug;
use serde_json::{Value as Json, json};

/// Where `enabled = true` sends traces when no endpoint is set anywhere.
const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:4318";

/// The default budget for one export.
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);

/// Where and how traces are sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OtlpHttp {
    host: String,
    port: u16,
    path: String,
    headers: Vec<(String, String)>,
    resource: Vec<(String, String)>,
    timeout: Duration,
}

/// Install the exporter when the environment or the user's config file
/// asks for traces. `background`: export from a thread of its own, for a
/// command that serves requests for as long as its client runs.
pub(crate) fn init(background: bool) {
    let file = FileConfig::load(&|key| std::env::var(key).ok());
    if let Some(exporter) = OtlpHttp::configure(&|key| std::env::var(key).ok(), &file) {
        if background {
            telemetry::install(Box::new(Background::spawn(exporter)));
        } else {
            telemetry::install(Box::new(exporter));
        }
    }
}

/// Whether `argv` runs a long-running command, which gets no root span of
/// its own and exports in the background.
pub(crate) fn long_running(argv: &[String]) -> bool {
    matches!(
        argv.get(1).map(String::as_str),
        Some("server" | "mcp" | "lsp-daemon")
    )
}

/// The `[telemetry]` table of the user's config file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FileConfig {
    enabled: Option<bool>,
    endpoint: Option<String>,
    service_name: Option<String>,
    timeout_ms: Option<u64>,
}

impl FileConfig {
    /// `$XDG_CONFIG_HOME/colab/config.toml`, else `~/.config/colab/config.toml`.
    fn path(var: &dyn Fn(&str) -> Option<String>) -> Option<std::path::PathBuf> {
        let set = |k: &str| var(k).filter(|v| !v.trim().is_empty());
        let base = match set("XDG_CONFIG_HOME") {
            Some(dir) => std::path::PathBuf::from(dir),
            None => std::path::PathBuf::from(set("HOME")?).join(".config"),
        };
        Some(base.join("colab").join("config.toml"))
    }

    /// The file's telemetry settings; empty when there is no file, or it
    /// cannot be read or parsed.
    fn load(var: &dyn Fn(&str) -> Option<String>) -> Self {
        let Some(path) = Self::path(var) else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).unwrap_or_else(|why| {
                debug!("telemetry: ignoring {}: {why}", path.display());
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                debug!("telemetry: cannot read {}: {e}", path.display());
                Self::default()
            }
        }
    }

    fn parse(text: &str) -> std::result::Result<Self, String> {
        let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
        let Some(table) = doc.get("telemetry") else {
            return Ok(Self::default());
        };
        let table = table.as_table_like().ok_or("`telemetry` is not a table")?;
        let string = |key: &str| -> std::result::Result<Option<String>, String> {
            match table.get(key) {
                None => Ok(None),
                Some(v) => v
                    .as_str()
                    .map(|s| Some(s.trim().to_string()).filter(|s| !s.is_empty()))
                    .ok_or_else(|| format!("telemetry.{key} is not a string")),
            }
        };
        let enabled = match table.get("enabled") {
            None => None,
            Some(v) => Some(v.as_bool().ok_or("telemetry.enabled is not a boolean")?),
        };
        let timeout_ms = match table.get("timeout_ms") {
            None => None,
            Some(v) => Some(
                v.as_integer()
                    .and_then(|n| u64::try_from(n).ok())
                    .ok_or("telemetry.timeout_ms is not a non-negative integer")?,
            ),
        };
        Ok(Self {
            enabled,
            endpoint: string("endpoint")?,
            service_name: string("service_name")?,
            timeout_ms,
        })
    }
}

/// An exporter that hands each trace to a thread of its own, so the
/// thread that ended the trace (an MCP request's) never waits on the
/// collector. Traces still queued when the process exits are lost.
struct Background {
    queue: std::sync::mpsc::Sender<Vec<SpanData>>,
}

impl Background {
    fn spawn(exporter: OtlpHttp) -> Self {
        let (queue, traces) = std::sync::mpsc::channel::<Vec<SpanData>>();
        let _ = std::thread::Builder::new()
            .name("colab-telemetry".into())
            .spawn(move || {
                for trace in traces {
                    telemetry::Exporter::export(&exporter, trace);
                }
            });
        Self { queue }
    }
}

impl telemetry::Exporter for Background {
    fn export(&self, trace: Vec<SpanData>) {
        let _ = self.queue.send(trace);
    }
}

/// The root span of a command line, `colab <subcommand>`, with the flag
/// names it was given (never their values). Long-running commands
/// (`server`, `mcp`, `lsp-daemon`) get none: each request they serve is a
/// trace of its own.
pub(crate) fn command_span(argv: &[String], subcommands: &[String]) -> telemetry::Span {
    if !telemetry::enabled() {
        return telemetry::Span::inert();
    }
    let sub = argv
        .get(1)
        .filter(|a| subcommands.iter().any(|s| s == *a))
        .map(String::as_str);
    if long_running(argv) {
        return telemetry::Span::inert();
    }
    let mut span = telemetry::root(match sub {
        Some(sub) => format!("colab {sub}"),
        None => "colab".to_string(),
    });
    if let Some(sub) = sub {
        span.set("colab.subcommand", sub);
        // habit's names for a CLI run (its `command` record).
        span.set("habit.subcommand", sub);
    }
    let mut flags: Vec<String> = argv.iter().skip(1).filter_map(|a| flag_name(a)).collect();
    flags.sort();
    flags.dedup();
    if !flags.is_empty() {
        span.set("colab.flags", flags.clone());
        span.set("habit.flags", flags);
    }
    span
}

/// A flag's name, without dashes or value: `--format=json` → `format`.
/// `None` for anything that is not a flag (a path, a rule, `-`).
fn flag_name(arg: &str) -> Option<String> {
    let name = arg.strip_prefix('-')?.trim_start_matches('-');
    let name = name.split('=').next().unwrap_or("");
    (!name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then(|| name.to_string())
}

/// Record a command's outcome on its root span.
pub(crate) fn finish(span: &mut telemetry::Span, outcome: &colab_core::Result<u8>) {
    match outcome {
        Ok(code) => span.set("process.exit.code", i64::from(*code)),
        Err(e) => {
            span.set("process.exit.code", i64::from(e.exit_code()));
            span.set("error.type", e.kind());
            span.fail(e.kind());
        }
    }
}

impl OtlpHttp {
    /// The exporter `var` (an environment lookup) alone configures, if any.
    #[cfg(test)]
    pub(crate) fn from_env(var: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        Self::configure(var, &FileConfig::default())
    }

    /// The exporter `var` (an environment lookup) and the user's config
    /// file configure, if any. The environment wins key by key.
    pub(crate) fn configure(
        var: &dyn Fn(&str) -> Option<String>,
        file: &FileConfig,
    ) -> Option<Self> {
        let get = |key: &str| {
            var(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let either = |traces: &str, all: &str| get(traces).or_else(|| get(all));
        if get("OTEL_SDK_DISABLED").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
            return None;
        }
        if get("OTEL_TRACES_EXPORTER").is_some_and(|v| !v.split(',').any(|e| e.trim() == "otlp")) {
            return None;
        }
        if file.enabled == Some(false) {
            return None;
        }
        let url = match get("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") {
            Some(url) => url,
            None => {
                // `enabled = true` with no endpoint anywhere: the local
                // OTLP/HTTP default.
                let base = get("OTEL_EXPORTER_OTLP_ENDPOINT")
                    .or_else(|| file.endpoint.clone())
                    .or_else(|| {
                        (file.enabled == Some(true)).then(|| DEFAULT_ENDPOINT.to_string())
                    })?;
                let base = base.trim_end_matches('/');
                if base.ends_with("/v1/traces") {
                    base.to_string()
                } else {
                    format!("{base}/v1/traces")
                }
            }
        };
        let Some(rest) = url.strip_prefix("http://") else {
            debug!(
                "telemetry: {url} is not plain http://; colab exports only OTLP/HTTP without TLS"
            );
            return None;
        };
        let (authority, path) = match rest.find('/') {
            Some(at) => (&rest[..at], rest[at..].to_string()),
            None => (rest, "/v1/traces".to_string()),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), port.parse::<u16>().ok()?),
            None => (authority.to_string(), 80),
        };
        let protocol = either(
            "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
            "OTEL_EXPORTER_OTLP_PROTOCOL",
        );
        // OTLP's gRPC and HTTP ports are 4317 and 4318; a gRPC endpoint is
        // reached on the same host's HTTP one.
        let port = if port == 4317 || protocol.as_deref() == Some("grpc") {
            4318
        } else {
            port
        };
        let headers = pairs(
            &either(
                "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
                "OTEL_EXPORTER_OTLP_HEADERS",
            )
            .unwrap_or_default(),
        );
        let timeout = either(
            "OTEL_EXPORTER_OTLP_TRACES_TIMEOUT",
            "OTEL_EXPORTER_OTLP_TIMEOUT",
        )
        .and_then(|ms| ms.parse::<u64>().ok())
        .or(file.timeout_ms)
        .map_or(DEFAULT_TIMEOUT, Duration::from_millis);

        // OTEL_SERVICE_NAME wins over a service.name in the attributes.
        let mut resource = pairs(&get("OTEL_RESOURCE_ATTRIBUTES").unwrap_or_default());
        resource.retain(|(k, _)| k != "service.version");
        let service = get("OTEL_SERVICE_NAME")
            .or_else(|| {
                resource
                    .iter()
                    .find(|(k, _)| k == "service.name")
                    .map(|(_, v)| v.clone())
            })
            .or_else(|| file.service_name.clone())
            .unwrap_or_else(|| "colab".to_string());
        resource.retain(|(k, _)| k != "service.name");
        resource.insert(0, ("service.name".into(), service));
        resource.insert(
            1,
            ("service.version".into(), crate::cli::VERSION.to_string()),
        );
        if !resource.iter().any(|(k, _)| k == "vcs.repository.name")
            && let Some(repo) = std::env::current_dir()
                .ok()
                .and_then(|d| repository_name(&d))
        {
            resource.push(("vcs.repository.name".into(), repo));
        }
        Some(Self {
            host,
            port,
            path,
            headers,
            resource,
            timeout,
        })
    }

    /// The OTLP/JSON request for one trace.
    pub(crate) fn body(&self, trace: &[SpanData]) -> Json {
        let mut resource = self.resource.clone();
        if !resource.iter().any(|(k, _)| k == "habit.kind")
            && let Some(kind) = habit_kind(trace)
        {
            resource.push(("habit.kind".into(), kind.into()));
        }
        let attributes: Vec<Json> = resource
            .iter()
            .map(|(k, v)| json!({"key": k, "value": {"stringValue": v}}))
            .collect();
        json!({
            "resourceSpans": [{
                "resource": {"attributes": attributes},
                "scopeSpans": [{
                    "scope": {"name": "colab", "version": crate::cli::VERSION},
                    "spans": trace.iter().map(span_json).collect::<Vec<_>>(),
                }],
            }],
        })
    }

    fn post(&self, body: &str) -> std::io::Result<u16> {
        let address = (self.host.as_str(), self.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| std::io::Error::other("the endpoint's host does not resolve"))?;
        let mut stream = TcpStream::connect_timeout(&address, self.timeout)?;
        stream.set_write_timeout(Some(self.timeout))?;
        stream.set_read_timeout(Some(self.timeout))?;
        let mut request = format!(
            "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.path,
            self.host,
            self.port,
            body.len()
        );
        for (k, v) in &self.headers {
            request.push_str(k);
            request.push_str(": ");
            request.push_str(v);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes())?;
        stream.write_all(body.as_bytes())?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head)?;
        // `HTTP/1.1 200`
        std::str::from_utf8(&head[9..12])
            .ok()
            .and_then(|code| code.parse().ok())
            .ok_or_else(|| std::io::Error::other("the endpoint did not answer HTTP"))
    }
}

impl telemetry::Exporter for OtlpHttp {
    fn export(&self, trace: Vec<SpanData>) {
        let body = self.body(&trace).to_string();
        match self.post(&body) {
            Ok(code) if (200..300).contains(&code) => {}
            Ok(code) => debug!("telemetry: the collector answered {code}"),
            Err(e) => debug!(
                "telemetry: export to {}:{} failed: {e}",
                self.host, self.port
            ),
        }
    }
}

/// What a trace is to habit, by its root (`habit.kind` on the resource):
/// a CLI run is a `command`, an MCP tool call `mcp`; anything else (a
/// `colab server` walk) is a plain span. Other collectors ignore it.
fn habit_kind(trace: &[SpanData]) -> Option<&'static str> {
    let root = trace.iter().find(|s| s.parent.is_none())?;
    let has = |key: &str| root.attributes.iter().any(|(k, _)| k == key);
    if has("colab.mcp.tool") {
        Some("mcp")
    } else if has("process.exit.code") {
        Some("command")
    } else {
        None
    }
}

fn span_json(span: &SpanData) -> Json {
    let nanos = |t: SystemTime| {
        t.duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
            .to_string()
    };
    let mut out = json!({
        "traceId": hex(&span.trace_id),
        "spanId": hex(&span.span_id),
        "name": span.name,
        // SPAN_KIND_INTERNAL / SPAN_KIND_CLIENT
        "kind": match span.kind { Kind::Internal => 1, Kind::Client => 3 },
        "startTimeUnixNano": nanos(span.start),
        "endTimeUnixNano": nanos(span.end),
        "attributes": span.attributes.iter().map(|(k, v)| json!({"key": k, "value": value_json(v)})).collect::<Vec<_>>(),
    });
    if let Some(parent) = &span.parent {
        out["parentSpanId"] = json!(hex(parent));
    }
    if let Some(why) = &span.error {
        // STATUS_CODE_ERROR
        out["status"] = json!({"code": 2, "message": why});
    }
    out
}

fn value_json(value: &Value) -> Json {
    match value {
        Value::Str(s) => json!({"stringValue": s}),
        // OTLP/JSON carries 64-bit integers as strings.
        Value::Int(n) => json!({"intValue": n.to_string()}),
        Value::Bool(b) => json!({"boolValue": b}),
        Value::Strs(list) => {
            json!({"arrayValue": {"values": list.iter().map(|s| json!({"stringValue": s})).collect::<Vec<_>>()}})
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// `k=v,k2=v2` (W3C baggage-style, values percent-decoded), as
/// `OTEL_RESOURCE_ATTRIBUTES` and `OTEL_EXPORTER_OTLP_HEADERS` are written.
fn pairs(text: &str) -> Vec<(String, String)> {
    text.split(',')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let k = k.trim();
            (!k.is_empty()).then(|| (k.to_string(), percent_decode(v.trim())))
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex_pair = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex_pair) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The name of the git repository `dir` is in (its top directory's name,
/// never the path), as `vcs.repository.name`.
fn repository_name(dir: &Path) -> Option<String> {
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn configured(vars: &[(&str, &str)]) -> Option<OtlpHttp> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        OtlpHttp::from_env(&|k| vars.get(k).cloned())
    }

    fn with_file(vars: &[(&str, &str)], file: &str) -> Option<OtlpHttp> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let file = FileConfig::parse(file).expect("a valid config file");
        OtlpHttp::configure(&|k| vars.get(k).cloned(), &file)
    }

    #[test]
    fn the_config_file_alone_turns_export_on() {
        let otlp = with_file(
            &[],
            "[telemetry]\nendpoint = \"http://127.0.0.1:4318\"\nservice_name = \"colab-dev\"\ntimeout_ms = 150\n",
        )
        .unwrap();
        assert_eq!(
            (otlp.host.as_str(), otlp.port, otlp.path.as_str()),
            ("127.0.0.1", 4318, "/v1/traces")
        );
        assert_eq!(otlp.resource[0].1, "colab-dev");
        assert_eq!(otlp.timeout, Duration::from_millis(150));
        // A file naming a gRPC port is reached on OTLP/HTTP's, as the env is.
        let grpc = with_file(&[], "[telemetry]\nendpoint = \"http://127.0.0.1:4317/\"\n").unwrap();
        assert_eq!(grpc.port, 4318);
        // A base URL that already names the traces path is not doubled.
        let full = with_file(&[], "[telemetry]\nendpoint = \"http://h:9000/v1/traces\"\n").unwrap();
        assert_eq!(full.path, "/v1/traces");
    }

    #[test]
    fn the_environment_wins_over_the_file_key_by_key() {
        let file = "[telemetry]\nendpoint = \"http://file:4318\"\nservice_name = \"from-file\"\ntimeout_ms = 150\n";
        let otlp = with_file(
            &[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://env:4318"),
                ("OTEL_EXPORTER_OTLP_TIMEOUT", "300"),
            ],
            file,
        )
        .unwrap();
        assert_eq!(otlp.host, "env");
        assert_eq!(otlp.timeout, Duration::from_millis(300));
        // Keys the environment leaves unset come from the file.
        assert_eq!(otlp.resource[0].1, "from-file");
        let named = with_file(&[("OTEL_SERVICE_NAME", "from-env")], file).unwrap();
        assert_eq!(
            (named.host.as_str(), named.resource[0].1.as_str()),
            ("file", "from-env")
        );
    }

    #[test]
    fn disabled_in_the_file_or_the_environment_is_off_whatever_else_is_set() {
        let env_endpoint = ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318");
        let off = "[telemetry]\nenabled = false\nendpoint = \"http://127.0.0.1:4318\"\n";
        assert!(with_file(&[env_endpoint], off).is_none());
        assert!(with_file(&[], off).is_none());
        let on = "[telemetry]\nenabled = true\nendpoint = \"http://127.0.0.1:4318\"\n";
        assert!(with_file(&[], on).is_some());
        assert!(with_file(&[("OTEL_SDK_DISABLED", "true")], on).is_none());
        assert!(with_file(&[("OTEL_TRACES_EXPORTER", "none")], on).is_none());
        // Enabled without an endpoint anywhere: the local OTLP/HTTP default.
        let local = with_file(&[], "[telemetry]\nenabled = true\n").unwrap();
        assert_eq!(
            (local.host.as_str(), local.port, local.path.as_str()),
            ("127.0.0.1", 4318, "/v1/traces")
        );
        // An endpoint in the environment still wins over that default.
        let env = with_file(
            &[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://env:4318")],
            "[telemetry]\nenabled = true\n",
        )
        .unwrap();
        assert_eq!(env.host, "env");
        // Neither enabled nor an endpoint: off.
        assert!(with_file(&[], "[telemetry]\n").is_none());
    }

    #[test]
    fn a_malformed_or_missing_file_is_ignored() {
        assert!(FileConfig::parse("[telemetry\nendpoint = ").is_err());
        assert!(FileConfig::parse("[telemetry]\nenabled = \"yes\"\n").is_err());
        assert!(FileConfig::parse("telemetry = 3\n").is_err());
        assert_eq!(
            FileConfig::parse("[other]\nx = 1\n"),
            Ok(FileConfig::default())
        );

        let dir = std::env::temp_dir().join(format!("colab-telemetry-{}", std::process::id()));
        let config = dir.join("colab");
        std::fs::create_dir_all(&config).unwrap();
        let lookup = |k: &str| (k == "XDG_CONFIG_HOME").then(|| dir.to_string_lossy().into_owned());
        // Missing: no telemetry config.
        assert_eq!(FileConfig::load(&lookup), FileConfig::default());
        // Malformed: ignored, not an error.
        std::fs::write(config.join("config.toml"), "[telemetry\n").unwrap();
        assert_eq!(FileConfig::load(&lookup), FileConfig::default());
        std::fs::write(
            config.join("config.toml"),
            "[telemetry]\nendpoint = \"http://127.0.0.1:4318\"\n",
        )
        .unwrap();
        assert_eq!(
            FileConfig::load(&lookup).endpoint.as_deref(),
            Some("http://127.0.0.1:4318")
        );
        // Without XDG_CONFIG_HOME, HOME's .config.
        let home = |k: &str| (k == "HOME").then(|| "/h".to_string());
        assert_eq!(
            FileConfig::path(&home).unwrap(),
            std::path::Path::new("/h/.config/colab/config.toml")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_running_commands_export_in_the_background() {
        let argv = |a: &[&str]| a.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert!(long_running(&argv(&["colab", "mcp"])));
        assert!(long_running(&argv(&["colab", "server"])));
        assert!(!long_running(&argv(&["colab", "find"])));
        // The background exporter returns at once, even with nothing listening.
        let otlp = configured(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:9"),
            ("OTEL_EXPORTER_OTLP_TIMEOUT", "2000"),
        ])
        .unwrap();
        let background = Background::spawn(otlp);
        let started = std::time::Instant::now();
        telemetry::Exporter::export(&background, Vec::new());
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn off_without_an_endpoint_or_when_disabled() {
        assert!(configured(&[]).is_none());
        let endpoint = ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318");
        assert!(configured(&[endpoint]).is_some());
        assert!(configured(&[endpoint, ("OTEL_SDK_DISABLED", "true")]).is_none());
        assert!(configured(&[endpoint, ("OTEL_TRACES_EXPORTER", "none")]).is_none());
        assert!(configured(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "https://x.example")]).is_none());
    }

    #[test]
    fn a_grpc_endpoint_is_reached_on_its_hosts_otlp_http_port() {
        // What Claude Code's settings set, and colab inherits.
        let otlp = configured(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4317"),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
        ])
        .unwrap();
        assert_eq!(
            (otlp.host.as_str(), otlp.port, otlp.path.as_str()),
            ("127.0.0.1", 4318, "/v1/traces")
        );
        let traces = configured(&[(
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "http://collector:9000/custom/traces",
        )])
        .unwrap();
        assert_eq!(
            (traces.port, traces.path.as_str()),
            (9000, "/custom/traces")
        );
    }

    #[test]
    fn resource_and_headers_come_from_the_standard_variables() {
        let otlp = configured(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318/"),
            (
                "OTEL_RESOURCE_ATTRIBUTES",
                "deployment.environment=dev,team=dev%20tools,service.name=x",
            ),
            ("OTEL_EXPORTER_OTLP_HEADERS", "x-token=abc"),
            ("OTEL_EXPORTER_OTLP_TIMEOUT", "250"),
        ])
        .unwrap();
        assert_eq!(otlp.resource[0], ("service.name".into(), "x".into()));
        assert_eq!(otlp.resource[1].0, "service.version");
        assert!(otlp.resource.contains(&("team".into(), "dev tools".into())));
        assert_eq!(otlp.headers, [("x-token".to_string(), "abc".to_string())]);
        assert_eq!(otlp.timeout, Duration::from_millis(250));
        let named = configured(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://h:4318"),
            ("OTEL_SERVICE_NAME", "colab-ci"),
        ])
        .unwrap();
        assert_eq!(named.resource[0].1, "colab-ci");
    }

    #[test]
    fn flag_names_never_values() {
        assert_eq!(flag_name("--format=json").as_deref(), Some("format"));
        assert_eq!(flag_name("-e").as_deref(), Some("e"));
        assert_eq!(flag_name("-"), None);
        assert_eq!(flag_name("src/main.rs"), None);
        assert_eq!(flag_name("--path=/secret dir"), Some("path".into()));
    }

    #[test]
    fn a_trace_is_sent_as_otlp_json_and_parented() {
        let otlp = configured(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318")]).unwrap();
        let start = SystemTime::now();
        let root = SpanData {
            trace_id: [1; 16],
            span_id: [2; 8],
            parent: None,
            name: "colab refactor".into(),
            kind: Kind::Internal,
            start,
            end: start,
            attributes: vec![("process.exit.code".into(), Value::Int(11))],
            error: Some("config".into()),
        };
        let child = SpanData {
            span_id: [3; 8],
            parent: Some([2; 8]),
            name: "lsp textDocument/hover".into(),
            kind: Kind::Client,
            attributes: vec![("colab.flags".into(), Value::Strs(vec!["write".into()]))],
            error: None,
            ..root.clone()
        };
        let body = otlp.body(&[root, child]);
        let spans = &body["resourceSpans"][0]["scopeSpans"][0]["spans"];
        assert_eq!(spans[0]["traceId"], "01".repeat(16));
        assert_eq!(spans[0]["status"]["code"], 2);
        assert_eq!(spans[0]["attributes"][0]["value"]["intValue"], "11");
        assert!(spans[0].get("parentSpanId").is_none());
        assert_eq!(spans[1]["parentSpanId"], "02".repeat(8));
        assert_eq!(spans[1]["kind"], 3);
        assert_eq!(
            spans[1]["attributes"][0]["value"]["arrayValue"]["values"][0]["stringValue"],
            "write"
        );
        let resource = &body["resourceSpans"][0]["resource"]["attributes"];
        assert_eq!(resource[0]["value"]["stringValue"], "colab");
        // A command's trace is habit's `command` record.
        assert!(
            resource
                .to_string()
                .contains(r#""key":"habit.kind","value":{"stringValue":"command"}"#),
            "{resource}"
        );
    }

    #[test]
    fn export_posts_the_trace_to_the_collector() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            // Read until the body named by Content-Length has arrived.
            loop {
                let n = stream.read(&mut buf).unwrap();
                seen.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&seen);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .and_then(|n| n.parse().ok())
                        .unwrap();
                    if body.len() >= len {
                        break;
                    }
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            String::from_utf8(seen).unwrap()
        });
        let endpoint = format!("http://127.0.0.1:{port}");
        let otlp = configured(&[("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint)]).unwrap();
        let start = SystemTime::now();
        telemetry::Exporter::export(
            &otlp,
            vec![SpanData {
                trace_id: [7; 16],
                span_id: [8; 8],
                parent: None,
                name: "colab find".into(),
                kind: Kind::Internal,
                start,
                end: start,
                attributes: Vec::new(),
                error: None,
            }],
        );
        let request = server.join().unwrap();
        assert!(
            request.starts_with("POST /v1/traces HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.contains("Content-Type: application/json"),
            "{request}"
        );
        assert!(request.contains("\"name\":\"colab find\""), "{request}");
    }

    #[test]
    fn an_unreachable_collector_is_dropped_quietly_and_quickly() {
        // Nothing listens on port 9 (discard) on loopback.
        let otlp = configured(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:9"),
            ("OTEL_EXPORTER_OTLP_TIMEOUT", "200"),
        ])
        .unwrap();
        let started = std::time::Instant::now();
        telemetry::Exporter::export(&otlp, Vec::new());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
