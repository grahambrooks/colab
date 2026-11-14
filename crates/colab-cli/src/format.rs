//! Output formats for `colab refactor`.
//!
//! Each [`Format`] picks a [`Reporter`] that consumes [`FileChange`]
//! events from the walker and writes to stdout. Diagnostic log lines
//! (`Processing …`) still go to stderr via `env_logger`, so a pipeline
//! like `colab refactor --format json | jq` is safe.
//!
//! Every reporter is handed the final [`RunReport`] so it can emit the
//! aggregate counters and the per-rule match counts. A rule that matched
//! zero files is surfaced by all of them — it compiled and ran but never
//! changed a byte, which is nearly always a bug in the script.
//!
//! Unchanged files produce no output on any format. They survive only as
//! the `scanned` / `changed` counters in the summary.

use std::io::{self, Write};

use clap::ValueEnum;
use colab_core::fileops::FileOp;
use colab_core::render::{self, ChangeCollector, Detail, RenderOptions};
use colab_core::report::RunReport;
use colab_core::walker::FileChange;
use serde_json::json;

/// Output format for `colab refactor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Coloured log lines plus a summary. Default for TTY stdout.
    Human,
    /// One JSON document for the whole run: summary, per-rule counts,
    /// changed paths, and (at `--detail diff`) capped diffs.
    Json,
    /// One JSON object per *changed* file, newline-separated, then a
    /// final summary object. For streaming pipelines.
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

    /// Detail level to use when `--detail` was not given.
    ///
    /// `--format diff` asked for diffs by naming the format, so it
    /// implies `Detail::Diff`; everything else defaults to `counts`,
    /// which describes the blast radius without paying for hunks.
    pub fn default_detail(self) -> Detail {
        match self {
            Format::Diff => Detail::Diff,
            _ => Detail::Counts,
        }
    }
}

/// Stream consumer of [`FileChange`] events plus a final [`RunReport`].
pub trait Reporter {
    /// One file operation, reported before any [`report`](Self::report)
    /// of an edit, in the order the operations are made.
    fn file_op(&mut self, _op: &FileOp) -> io::Result<()> {
        Ok(())
    }
    fn report(&mut self, change: &FileChange) -> io::Result<()>;
    /// The run was refused (conflicts), so report the changes still to
    /// come as "would" rather than "wrote". Only wording depends on it.
    fn refused(&mut self) {}
    /// Emit whatever the format shows at the end of a run, then flush.
    /// Called exactly once, after the last `report`.
    fn finish(&mut self, report: &RunReport) -> io::Result<()>;
}

pub fn make_reporter(format: Format, mode: ExecMode, options: RenderOptions) -> Box<dyn Reporter> {
    match format {
        Format::Human => Box::new(HumanReporter::new(mode, options)),
        Format::Json => Box::new(JsonReporter::new(options)),
        Format::Ndjson => Box::new(NdjsonReporter::new(options)),
        Format::Diff => Box::new(DiffReporter::new()),
    }
}

/// Human-friendly reporter. Per-file lines go to the log (stderr); the
/// summary and any advisories go to **stdout**, so an agent shelling out
/// and capturing stdout sees a result rather than nothing.
pub struct HumanReporter {
    mode: ExecMode,
    options: RenderOptions,
    out: io::Stdout,
}

impl HumanReporter {
    pub fn new(mode: ExecMode, options: RenderOptions) -> Self {
        Self {
            mode,
            options,
            out: io::stdout(),
        }
    }
}

impl Reporter for HumanReporter {
    fn refused(&mut self) {
        self.mode = ExecMode::DryRun;
    }

    fn file_op(&mut self, op: &FileOp) -> io::Result<()> {
        if self.options.detail == Detail::Summary {
            return Ok(());
        }
        match self.mode {
            ExecMode::Write => log::info!("Made {}", op),
            ExecMode::DryRun | ExecMode::Check => log::info!("Would {}", op),
        }
        Ok(())
    }

    fn report(&mut self, change: &FileChange) -> io::Result<()> {
        if !change.changed() {
            log::debug!("No changes for {}", change.path.display());
            return Ok(());
        }
        if self.options.detail == Detail::Summary {
            return Ok(());
        }
        match self.mode {
            ExecMode::Write => log::info!("Wrote {}", change.path.display()),
            ExecMode::DryRun | ExecMode::Check => {
                log::info!("Would change {}", change.path.display())
            }
        }
        Ok(())
    }

    fn finish(&mut self, report: &RunReport) -> io::Result<()> {
        writeln!(
            self.out,
            "{} file(s) scanned, {} changed, {} → {} bytes in {} ms",
            report.files_scanned,
            report.files_changed,
            report.bytes_before,
            report.bytes_after,
            report.elapsed.as_millis()
        )?;
        if !report.file_ops.is_empty() {
            writeln!(
                self.out,
                "{} file operation(s) made through git",
                report.file_ops.len()
            )?;
        }

        if self.options.detail != Detail::Summary {
            for rule in &report.rules {
                writeln!(
                    self.out,
                    "  rule {}: {} file(s) — {}",
                    rule.index + 1,
                    rule.files_matched,
                    rule.rule
                )?;
            }
        }

        for op in &report.file_ops {
            writeln!(self.out, "  {}", op)?;
        }
        for skipped in &report.skipped {
            writeln!(
                self.out,
                "  skipped {}: {}",
                skipped.path.display(),
                skipped.reason
            )?;
        }
        for conflict in render::conflict_lines(report) {
            writeln!(self.out, "conflict: {}", conflict)?;
        }
        for advisory in render::advisories(report) {
            writeln!(self.out, "warning: {}", advisory)?;
        }
        self.out.flush()
    }
}

/// One JSON document for the whole run: aggregate counters, per-rule
/// counts, changed paths, and capped diffs. Compact, not pretty-printed —
/// indentation is pure cost to a machine reader.
pub struct JsonReporter {
    collector: ChangeCollector,
    out: io::Stdout,
}

impl JsonReporter {
    pub fn new(options: RenderOptions) -> Self {
        Self {
            collector: ChangeCollector::new(options),
            out: io::stdout(),
        }
    }
}

impl Reporter for JsonReporter {
    fn report(&mut self, change: &FileChange) -> io::Result<()> {
        self.collector.push(change);
        Ok(())
    }

    fn finish(&mut self, report: &RunReport) -> io::Result<()> {
        let mut value = self.collector.to_json(report);
        let advisories = render::advisories(report);
        if !advisories.is_empty()
            && let Some(obj) = value.as_object_mut()
        {
            obj.insert("warnings".to_string(), json!(advisories));
        }
        writeln!(self.out, "{}", value)?;
        self.out.flush()
    }
}

/// One JSON object per changed file, then a final summary object.
/// Unchanged files are not emitted.
pub struct NdjsonReporter {
    options: RenderOptions,
    out: io::Stdout,
}

impl NdjsonReporter {
    pub fn new(options: RenderOptions) -> Self {
        Self {
            options,
            out: io::stdout(),
        }
    }
}

impl Reporter for NdjsonReporter {
    fn file_op(&mut self, op: &FileOp) -> io::Result<()> {
        if self.options.detail == Detail::Summary {
            return Ok(());
        }
        let value = match op {
            FileOp::Move { from, to } => {
                json!({"type": "move", "from": from.to_string_lossy(), "to": to.to_string_lossy()})
            }
            FileOp::Create { path } => json!({"type": "create", "path": path.to_string_lossy()}),
            FileOp::Delete { path, .. } => {
                json!({"type": "delete", "path": path.to_string_lossy()})
            }
        };
        writeln!(self.out, "{}", value)
    }

    fn report(&mut self, change: &FileChange) -> io::Result<()> {
        if !change.changed() || self.options.detail == Detail::Summary {
            return Ok(());
        }
        let mut value = json!({
            "type": "file",
            "path": change.path.to_string_lossy(),
            "bytes_before": change.before.len(),
            "bytes_after": change.after.len(),
            "rules": change.rules_fired,
        });
        if self.options.detail == Detail::Diff
            && let Some(obj) = value.as_object_mut()
        {
            obj.insert(
                "diff".to_string(),
                json!(render::unified_diff(
                    &change.path,
                    &change.before,
                    &change.after
                )),
            );
        }
        writeln!(self.out, "{}", value)
    }

    fn finish(&mut self, report: &RunReport) -> io::Result<()> {
        let mut value = json!({
            "type": "summary",
            "summary": render::summary_json(report),
            "rules": render::rules_json(report),
        });
        if let Some(conflicts) = render::conflicts_json(report) {
            value["conflicts"] = conflicts;
        }
        let advisories = render::advisories(report);
        if !advisories.is_empty()
            && let Some(obj) = value.as_object_mut()
        {
            obj.insert("warnings".to_string(), json!(advisories));
        }
        writeln!(self.out, "{}", value)?;
        self.out.flush()
    }
}

/// Unified diff per changed file. Intentionally uncapped and free of any
/// summary so the output stays valid `patch` input.
pub struct DiffReporter {
    out: io::Stdout,
    /// Where each moved or created file came from (`None`: created), so
    /// its edits diff against the old path, as `git apply` expects.
    origins: std::collections::HashMap<std::path::PathBuf, Option<std::path::PathBuf>>,
    /// Rename / new-file headers not yet written, by the file's final
    /// path. Each is written immediately above that file's hunks — one
    /// `diff --git` block per file — or, for a pure move, at the end.
    pending: Vec<(std::path::PathBuf, FileOp)>,
}

impl DiffReporter {
    pub fn new() -> Self {
        Self {
            out: io::stdout(),
            origins: Default::default(),
            pending: Vec::new(),
        }
    }
}

impl Default for DiffReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Reporter for DiffReporter {
    fn file_op(&mut self, op: &FileOp) -> io::Result<()> {
        match op {
            FileOp::Move { from, to } => {
                let origin = self.origins.remove(from).unwrap_or(Some(from.clone()));
                self.origins.insert(to.clone(), origin);
                self.pending.push((to.clone(), op.clone()));
                Ok(())
            }
            FileOp::Create { path } => {
                self.origins.insert(path.clone(), None);
                self.pending.push((path.clone(), op.clone()));
                Ok(())
            }
            FileOp::Delete { .. } => render::write_file_op_header(&mut self.out, op, false),
        }
    }

    fn report(&mut self, change: &FileChange) -> io::Result<()> {
        if !change.changed() {
            return Ok(());
        }
        if let Some(i) = self.pending.iter().position(|(p, _)| *p == change.path) {
            let (_, op) = self.pending.remove(i);
            render::write_file_op_header(&mut self.out, &op, false)?;
        }
        match self.origins.get(&change.path) {
            Some(origin) => render::write_unified_diff_between(
                &mut self.out,
                origin.as_deref(),
                &change.path,
                &change.before,
                &change.after,
            ),
            None => render::write_unified_diff(
                &mut self.out,
                &change.path,
                &change.before,
                &change.after,
            ),
        }
    }

    fn finish(&mut self, _report: &RunReport) -> io::Result<()> {
        // Moves and creates with no content change.
        for (_, op) in std::mem::take(&mut self.pending) {
            render::write_file_op_header(&mut self.out, &op, true)?;
        }
        // No summary: this output is meant to be piped to `git apply`.
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> RunReport {
        let mut report = RunReport::with_rules(["go::import \"a\" -> \"b\"", "go::symbol dead"]);
        report.files_visited = 10;
        report.files_scanned = 4;
        report.files_changed = 1;
        report.rules[0].files_matched = 1;
        report
    }

    #[test]
    fn dead_rules_are_advertised() {
        let advisories = render::advisories(&report());
        assert_eq!(advisories.len(), 1);
        assert!(advisories[0].contains("matched no files"));
    }

    #[test]
    fn a_run_that_changed_nothing_explains_itself() {
        let mut report = report();
        report.files_changed = 0;
        report.rules[0].files_matched = 0;
        let advisories = render::advisories(&report);
        assert!(advisories.iter().any(|a| a.contains("no rule matched")));
    }

    #[test]
    fn diff_format_implies_diff_detail() {
        assert_eq!(Format::Diff.default_detail(), Detail::Diff);
        assert_eq!(Format::Json.default_detail(), Detail::Counts);
        assert_eq!(Format::Human.default_detail(), Detail::Counts);
    }

    #[test]
    fn human_writes_on_a_tty_and_dry_runs_otherwise() {
        assert_eq!(Format::Human.default_exec_mode(true), ExecMode::Write);
        assert_eq!(Format::Human.default_exec_mode(false), ExecMode::DryRun);
        assert_eq!(Format::Json.default_exec_mode(true), ExecMode::DryRun);
    }
}
