//! Progress for the long tool calls: `colab.preview` / `colab.apply` with
//! a `progressToken` send `notifications/progress` while they walk.

use super::*;

/// One progress notification per N files. Hand-tuned to be small
/// enough that 1k-file runs emit ≥10 ticks but not so frequent
/// that the wire becomes the bottleneck.
pub(crate) const PROGRESS_BATCH: u64 = 64;

pub(crate) fn emit_progress<W: Write>(
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

/// Where per-file progress goes while a script runs.
///
/// The streaming and synchronous entry points differ *only* in this, so
/// modelling it as a sink keeps one implementation of the run itself.
/// When these were two functions they were 61% identical, and a change to
/// the report shape had to be made in both — a divergence would have
/// meant an agent that passes a `progressToken` seeing different
/// behaviour from one that does not.
pub(crate) trait ProgressSink {
    /// Called once per processed file.
    fn tick(&mut self, processed: u64);
    /// Called once when the run finishes, with the final count.
    fn finish(&mut self, total: u64);
}

/// The synchronous path: no notifications.
pub(crate) struct NoProgress;

impl ProgressSink for NoProgress {
    fn tick(&mut self, _processed: u64) {}
    fn finish(&mut self, _total: u64) {}
}

/// The streaming path: a `notifications/progress` message every
/// [`PROGRESS_BATCH`] files, plus a final 100% tick.
pub(crate) struct Notifications<'a, W: Write> {
    pub(crate) writer: &'a mut W,
    pub(crate) token: &'a Value,
    pub(crate) last_emitted: u64,
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
