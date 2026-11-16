//! Traces of colab's own work, in OpenTelemetry's model: a trace per
//! command (or per MCP tool call, or per `colab server` walk), its spans
//! the steps inside it — compiling the script, walking the tree, writing,
//! each language-server request.
//!
//! This module only records. The binary installs an [`Exporter`] (OTLP,
//! configured by the standard `OTEL_*` variables) when telemetry is
//! enabled; until then every call here is a no-op that allocates nothing,
//! so the library crates can be instrumented unconditionally.
//!
//! - [`root`] starts a trace; [`span`] starts a child of the span open on
//!   this thread, and is inert when none is.
//! - A span ends when its guard drops. When a trace's root ends, the trace
//!   is handed to the exporter, so a long-running server exports each
//!   request's trace as it finishes.
//! - Work on another thread joins the trace through [`Context`]: capture
//!   it with [`current`] and [`Context::enter`] it there (the walker does
//!   this for its workers).
//!
//! What a span may carry is deliberately narrow: names, counts, flag
//! names, method names and outcomes. Never file contents, paths, argument
//! values or script text.

use std::cell::RefCell;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

/// A span attribute's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    Strs(Vec<String>),
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Str(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Str(v)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Value::Int(i64::try_from(v).unwrap_or(i64::MAX))
    }
}
impl From<usize> for Value {
    fn from(v: usize) -> Self {
        Value::Int(i64::try_from(v).unwrap_or(i64::MAX))
    }
}
impl From<Vec<String>> for Value {
    fn from(v: Vec<String>) -> Self {
        Value::Strs(v)
    }
}

/// What a span does, in OpenTelemetry's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Work inside colab.
    Internal,
    /// A request colab makes of another process (a language server).
    Client,
}

/// A finished span, as the exporter receives it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpanData {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub parent: Option<[u8; 8]>,
    pub name: String,
    pub kind: Kind,
    pub start: SystemTime,
    pub end: SystemTime,
    pub attributes: Vec<(String, Value)>,
    /// `Some(description)` when the span's work failed.
    pub error: Option<String>,
}

/// Where finished traces go. Called once per trace, when its root ends,
/// on the thread that ended it; it must not block for long.
pub trait Exporter: Send + Sync {
    fn export(&self, trace: Vec<SpanData>);
}

static EXPORTER: OnceLock<Box<dyn Exporter>> = OnceLock::new();
/// Spans finished on any thread whose trace is still open.
static FINISHED: Mutex<Vec<SpanData>> = Mutex::new(Vec::new());
static IDS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The spans open on this thread, innermost last, as (trace, span).
    static OPEN: RefCell<Vec<([u8; 16], [u8; 8])>> = const { RefCell::new(Vec::new()) };
    /// Attributes [`annotate`]d onto an open span, by its id.
    static NOTES: RefCell<Vec<([u8; 8], String, Value)>> = const { RefCell::new(Vec::new()) };
}

/// Set an attribute on the innermost span open on this thread, for code
/// that has no handle on it (a command's root, from the report).
pub fn annotate(key: &str, value: impl Into<Value>) {
    if !enabled() {
        return;
    }
    if let Some((_, span_id)) = OPEN.with(|open| open.borrow().last().copied()) {
        NOTES.with(|notes| {
            notes
                .borrow_mut()
                .push((span_id, key.to_string(), value.into()))
        });
    }
}

/// Send every finished trace to `exporter` from now on. The first install
/// wins; returns whether this one did.
pub fn install(exporter: Box<dyn Exporter>) -> bool {
    EXPORTER.set(exporter).is_ok()
}

/// Whether an exporter is installed: spans are being recorded.
pub fn enabled() -> bool {
    EXPORTER.get().is_some()
}

/// A span. It ends, and is recorded, when dropped.
#[must_use = "a span ends when it is dropped"]
pub struct Span(Option<SpanData>);

/// Start a new trace whose root span is `name`.
pub fn root(name: impl Into<String>) -> Span {
    if !enabled() {
        return Span(None);
    }
    open(name.into(), random(), None, Kind::Internal)
}

/// Start a span inside the one open on this thread; inert when none is.
pub fn span(name: impl Into<String>) -> Span {
    child(name, Kind::Internal)
}

/// [`span`] for a request to another process.
pub fn client_span(name: impl Into<String>) -> Span {
    child(name, Kind::Client)
}

fn child(name: impl Into<String>, kind: Kind) -> Span {
    if !enabled() {
        return Span(None);
    }
    match OPEN.with(|open| open.borrow().last().copied()) {
        Some((trace, parent)) => open(name.into(), trace, Some(parent), kind),
        None => Span(None),
    }
}

fn open(name: String, trace_id: [u8; 16], parent: Option<[u8; 8]>, kind: Kind) -> Span {
    let span_id = random();
    OPEN.with(|open| open.borrow_mut().push((trace_id, span_id)));
    Span(Some(SpanData {
        trace_id,
        span_id,
        parent,
        name,
        kind,
        start: SystemTime::now(),
        end: SystemTime::now(),
        attributes: Vec::new(),
        error: None,
    }))
}

impl Span {
    /// A span that records nothing.
    pub fn inert() -> Self {
        Span(None)
    }

    /// Whether this span is being recorded (telemetry is on and it has a
    /// trace); compute costly attributes only when it is.
    pub fn is_recording(&self) -> bool {
        self.0.is_some()
    }

    /// Set attribute `key` to `value`, replacing an earlier value.
    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        if let Some(data) = &mut self.0 {
            let value = value.into();
            match data.attributes.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = value,
                None => data.attributes.push((key.to_string(), value)),
            }
        }
    }

    /// Mark the span's work as failed.
    pub fn fail(&mut self, why: impl fmt::Display) {
        if let Some(data) = &mut self.0 {
            data.error = Some(why.to_string());
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some(mut data) = self.0.take() else {
            return;
        };
        data.end = SystemTime::now();
        let notes: Vec<(String, Value)> = NOTES.with(|notes| {
            let mut notes = notes.borrow_mut();
            let (mine, rest): (Vec<_>, Vec<_>) =
                notes.drain(..).partition(|(id, _, _)| *id == data.span_id);
            *notes = rest;
            mine.into_iter().map(|(_, k, v)| (k, v)).collect()
        });
        for (key, value) in notes {
            match data.attributes.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => data.attributes.push((key, value)),
            }
        }
        OPEN.with(|open| {
            let mut open = open.borrow_mut();
            if let Some(at) = open.iter().rposition(|(_, id)| *id == data.span_id) {
                open.remove(at);
            }
        });
        let mut finished = FINISHED.lock().unwrap_or_else(PoisonError::into_inner);
        let is_root = data.parent.is_none();
        let trace_id = data.trace_id;
        finished.push(data);
        if !is_root {
            return;
        }
        let (trace, rest): (Vec<_>, Vec<_>) =
            finished.drain(..).partition(|s| s.trace_id == trace_id);
        *finished = rest;
        drop(finished);
        if let Some(exporter) = EXPORTER.get() {
            exporter.export(trace);
        }
    }
}

/// The span open on this thread, to continue its trace on another.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    trace_id: [u8; 16],
    span_id: [u8; 8],
}

/// The span open on this thread, if telemetry is on and one is.
pub fn current() -> Option<Context> {
    if !enabled() {
        return None;
    }
    OPEN.with(|open| open.borrow().last().copied())
        .map(|(trace_id, span_id)| Context { trace_id, span_id })
}

impl Context {
    /// Run `f` with this context's span as the parent of the spans it
    /// starts on this thread.
    pub fn enter<T>(&self, f: impl FnOnce() -> T) -> T {
        struct Leave([u8; 8]);
        impl Drop for Leave {
            fn drop(&mut self) {
                OPEN.with(|open| {
                    let mut open = open.borrow_mut();
                    if let Some(at) = open.iter().rposition(|(_, id)| *id == self.0) {
                        open.remove(at);
                    }
                });
            }
        }
        OPEN.with(|open| open.borrow_mut().push((self.trace_id, self.span_id)));
        let _leave = Leave(self.span_id);
        f()
    }
}

/// A random id, from the standard library's per-process hash keys, a
/// counter and the clock: unique enough for tracing, with no dependency.
fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    for (i, chunk) in out.chunks_mut(8).enumerate() {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(IDS.fetch_add(1, Ordering::Relaxed));
        hasher.write_usize(i);
        hasher.write_u128(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        let bytes = hasher.finish().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    if out.iter().all(|b| *b == 0) {
        out[0] = 1; // all zeros is "no id" in OpenTelemetry
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Collects every exported trace. One exporter for the whole test
    /// binary (the install is process-wide), so each test finds its own
    /// trace by its root's name.
    #[derive(Default)]
    struct Collect(Arc<Mutex<Vec<Vec<SpanData>>>>);

    impl Exporter for Collect {
        fn export(&self, trace: Vec<SpanData>) {
            self.0.lock().unwrap().push(trace);
        }
    }

    fn traces() -> Arc<Mutex<Vec<Vec<SpanData>>>> {
        static SEEN: OnceLock<Arc<Mutex<Vec<Vec<SpanData>>>>> = OnceLock::new();
        SEEN.get_or_init(|| {
            let seen = Arc::new(Mutex::new(Vec::new()));
            install(Box::new(Collect(seen.clone())));
            seen
        })
        .clone()
    }

    fn trace_named(root: &str) -> Vec<SpanData> {
        traces()
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.iter().any(|s| s.parent.is_none() && s.name == root))
            .cloned()
            .expect("the trace was exported")
    }

    #[test]
    fn a_trace_is_exported_when_its_root_ends_with_its_children() {
        traces();
        {
            let mut root = root("colab test-a");
            root.set("process.exit.code", 0_i64);
            let mut walk = span("walk");
            walk.set("colab.files.scanned", 3_usize);
            drop(walk);
            let _lsp = client_span("lsp textDocument/hover");
            drop(_lsp);
            annotate("colab.files.changed", 2_usize);
        }
        let trace = trace_named("colab test-a");
        assert_eq!(trace.len(), 3);
        let root = trace.iter().find(|s| s.parent.is_none()).unwrap();
        for child in trace.iter().filter(|s| s.parent.is_some()) {
            assert_eq!(child.parent, Some(root.span_id));
            assert_eq!(child.trace_id, root.trace_id);
        }
        let hover = trace.iter().find(|s| s.name.starts_with("lsp")).unwrap();
        assert_eq!(hover.kind, Kind::Client);
        assert!(
            root.attributes
                .contains(&("colab.files.changed".into(), Value::Int(2))),
            "annotated onto the innermost open span: {root:?}"
        );
    }

    #[test]
    fn a_span_without_an_open_parent_is_inert() {
        traces();
        let mut orphan = span("walk");
        assert!(!orphan.is_recording());
        orphan.set("ignored", true);
    }

    #[test]
    fn work_on_another_thread_joins_the_trace_through_its_context() {
        traces();
        {
            let _root = root("colab test-b");
            let context = current().unwrap();
            std::thread::spawn(move || {
                context.enter(|| {
                    let mut worker = span("worker");
                    worker.fail("it broke");
                })
            })
            .join()
            .unwrap();
        }
        let trace = trace_named("colab test-b");
        let worker = trace.iter().find(|s| s.name == "worker").unwrap();
        assert_eq!(worker.error.as_deref(), Some("it broke"));
        assert!(worker.parent.is_some());
    }

    #[test]
    fn ids_are_not_repeated() {
        let ids: std::collections::HashSet<[u8; 8]> = (0..1000).map(|_| random()).collect();
        assert_eq!(ids.len(), 1000);
    }
}
