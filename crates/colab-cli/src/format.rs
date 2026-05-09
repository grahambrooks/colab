//! Output formats for `colab refactor`.
//!
//! Each [`Format`] picks a [`Reporter`] that consumes [`FileChange`]
//! events from the walker and writes to stdout. Log lines (info/error)
//! go to stderr via `env_logger`, so a pipeline like
//! `colab refactor --format json | jq` is safe.
//!
//! Reporters also emit a final [`RunSummary`] so consumers can pick
//! up an aggregate without re-tallying per-file events. JSON and
//! NDJSON formats emit the summary as a `{"type": "summary", ...}`
//! line after the per-file events; the human format logs a final
//! one-liner; the diff format omits the summary so its output stays
//! valid as a `patch` input.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use clap::ValueEnum;
use colab_core::walker::FileChange;
use serde_json::json;

/// Output format for `colab refactor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Coloured log lines. Default for TTY stdout.
    Human,
    /// One JSON object per processed file (newline-separated).
    Json,
    /// Alias for `json`. Provided so scripts that expect ndjson can
    /// pass `--format ndjson` without surprise.
    Ndjson,
    /// Unified diff per changed file, suitable for `patch` or review UIs.
    Diff,
}

/// What `refactor` should do with its results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecMode {
    /// Apply changes in place.
    Write,
    /// Report what would change but do not write.
    DryRun,
    /// Like `DryRun`, but exit 10 if anything would change.
    Check,
}

impl Format {
    /// Default exec mode for a given format / TTY pairing, when neither
    /// `--write`, `--dry-run`, nor `--check` is supplied.
    pub fn default_exec_mode(self, stdout_is_tty: bool) -> ExecMode {
        match self {
            Format::Human if stdout_is_tty => ExecMode::Write,
            _ => ExecMode::DryRun,
        }
    }
}

/// Aggregate stats for one `colab refactor` invocation.
#[derive(Debug, Default, Clone)]
pub struct RunSummary {
    pub files_seen: u64,
    pub files_changed: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub elapsed: Duration,
}

impl RunSummary {
    pub fn record(&mut self, change: &FileChange) {
        self.files_seen += 1;
        self.bytes_before += change.before.len() as u64;
        self.bytes_after += change.after.len() as u64;
        if change.changed() {
            self.files_changed += 1;
        }
    }
}

/// Stream consumer of [`FileChange`] events plus a final
/// [`RunSummary`].
pub trait Reporter {
    fn report(&mut self, change: &FileChange) -> io::Result<()>;
    fn finish(&mut self) -> io::Result<()>;
    /// Emit the final run summary. Called once after the last
    /// `report`. The default impl is a no-op so reporters that do
    /// not summarise (e.g. the diff format) can opt out by not
    /// overriding it.
    fn report_summary(&mut self, _summary: &RunSummary) -> io::Result<()> {
        Ok(())
    }
}

pub fn make_reporter(format: Format, mode: ExecMode) -> Box<dyn Reporter> {
    match format {
        Format::Human => Box::new(HumanReporter { mode }),
        Format::Json | Format::Ndjson => Box::new(JsonReporter::new()),
        Format::Diff => Box::new(DiffReporter::new()),
    }
}

/// Human-friendly reporter. Each event becomes one log line; the
/// walker's own `Processing …` log line continues to fire from
/// elsewhere in the binary so pipelines see the same output as today.
pub struct HumanReporter {
    mode: ExecMode,
}

impl Reporter for HumanReporter {
    fn report(&mut self, change: &FileChange) -> io::Result<()> {
        if !change.changed() {
            log::debug!("No changes for {}", change.path.display());
            return Ok(());
        }
        match self.mode {
            ExecMode::Write => log::info!("Wrote {}", change.path.display()),
            ExecMode::DryRun => log::info!("Would change {}", change.path.display()),
            ExecMode::Check => log::info!("Would change {}", change.path.display()),
        }
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn report_summary(&mut self, s: &RunSummary) -> io::Result<()> {
        log::info!(
            "Done: {} file(s) seen, {} changed, {} → {} bytes in {} ms",
            s.files_seen,
            s.files_changed,
            s.bytes_before,
            s.bytes_after,
            s.elapsed.as_millis()
        );
        Ok(())
    }
}

/// One JSON object per file, newline-separated, then a final
/// `{"type": "summary", ...}` event with the aggregate stats.
///
/// File event shape:
///
/// ```json
/// {"type": "file", "path": "…", "changed": true,
///  "bytes_before": 42, "bytes_after": 48}
/// ```
pub struct JsonReporter {
    out: io::Stdout,
}

impl JsonReporter {
    pub fn new() -> Self {
        Self { out: io::stdout() }
    }
}

impl Default for JsonReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Reporter for JsonReporter {
    fn report(&mut self, change: &FileChange) -> io::Result<()> {
        let value = json!({
            "type": "file",
            "path": change.path.to_string_lossy(),
            "changed": change.changed(),
            "bytes_before": change.before.len(),
            "bytes_after": change.after.len(),
        });
        writeln!(self.out, "{}", value)
    }

    fn finish(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    fn report_summary(&mut self, s: &RunSummary) -> io::Result<()> {
        let value = json!({
            "type": "summary",
            "files_seen": s.files_seen,
            "files_changed": s.files_changed,
            "bytes_before": s.bytes_before,
            "bytes_after": s.bytes_after,
            "elapsed_ms": s.elapsed.as_millis() as u64,
        });
        writeln!(self.out, "{}", value)
    }
}

/// Unified diff per changed file. Intentionally does not emit a
/// summary so the output stays consumable by `patch`.
pub struct DiffReporter {
    out: io::Stdout,
}

impl DiffReporter {
    pub fn new() -> Self {
        Self { out: io::stdout() }
    }
}

impl Default for DiffReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Reporter for DiffReporter {
    fn report(&mut self, change: &FileChange) -> io::Result<()> {
        if !change.changed() {
            return Ok(());
        }
        write_unified_diff(&mut self.out, &change.path, &change.before, &change.after)
    }

    fn finish(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    // No `report_summary` override — diff output is meant to be
    // piped to `patch`, so we keep it free of trailing prose.
}

/// Write a unified diff for one file pair to `out`.
pub fn write_unified_diff<W: Write>(
    out: &mut W,
    path: &Path,
    before: &str,
    after: &str,
) -> io::Result<()> {
    let display = path.display();
    let header_a = format!("a/{}", display);
    let header_b = format!("b/{}", display);
    let diff = similar::TextDiff::from_lines(before, after);
    write!(out, "{}", diff.unified_diff().header(&header_a, &header_b))
}
