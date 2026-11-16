//! Conflict checks and the write step every run path ends with.

use super::*;

/// Exit code for a `--write` run that found conflicts and wrote nothing.
pub(super) const CONFLICT_EXIT: u8 = 11;

/// `true` (and a warning on the report) when the run found conflicts and
/// `--allow-conflicts` was not given, so nothing may be written.
pub(super) fn refuse_conflicts(args: &RefactorArgs, report: &mut RunReport) -> bool {
    if report.conflicts.is_empty() || args.allow_conflicts {
        return false;
    }
    report.warnings.push(format!(
        "wrote nothing: {} conflict(s) found; review them, then fix the rules \
         or rerun with --allow-conflicts",
        report.conflicts.len()
    ));
    true
}

/// The checks that run on a whole run's changes before anything is
/// written: Markdown links, then `--verify-lsp`.
pub(super) fn lsp_check(
    args: &RefactorArgs,
    refactoring: &codemod::Refactoring,
    pending: &[FileChange],
    report: &mut RunReport,
) -> Result<()> {
    if !args.no_link_check {
        let roots = if args.paths.is_empty() {
            vec![PathBuf::from(".")]
        } else {
            args.paths.clone()
        };
        crate::markdown::check_links(pending, &roots, report)?;
    }
    match args.verify_lsp.as_deref() {
        None => Ok(()),
        Some(command) => crate::lsp_commands::verify_lsp(
            Some(command).filter(|c| !c.is_empty()),
            pending,
            report,
            Some(&refactoring.rules),
        ),
    }
}

/// Report changes held back until the conflict check.
pub(super) fn report_all(
    reporter: &mut dyn format::Reporter,
    changes: &[FileChange],
) -> Result<()> {
    for change in changes {
        reporter
            .report(change)
            .map_err(|e| Error::io_at(&change.path, e))?;
    }
    Ok(())
}

/// Re-read every file the run would write, before any is written
/// ([`colab_core::merge::prepare_writes`]): a file changed on disk since
/// the run read it is merged with the run's change when their edits do
/// not overlap, and is a conflict when they do, so the conflict check
/// that follows refuses the write.
pub(super) fn prepare_writes(
    changes: &[FileChange],
    report: &mut RunReport,
) -> Result<colab_core::merge::Prepared> {
    let mut span = colab_core::telemetry::span("prepare writes");
    let prepared = colab_core::merge::prepare_writes(
        changes
            .iter()
            .filter(|c| c.changed())
            .map(|c| (c.path.as_path(), c.before.as_str(), c.after.as_str())),
    )?;
    span.set("colab.files.merged", prepared.merged.len());
    span.set("colab.conflicts", prepared.conflicts.len());
    report.warnings.extend(prepared.merge_warning());
    report.conflicts.extend(prepared.conflicts.iter().cloned());
    Ok(prepared)
}

/// Write what [`prepare_writes`] cleared, saving each file as it was on
/// disk under `backup` first. A file whose disk changes conflict with the
/// run's is never written, even under `--allow-conflicts`.
pub(super) fn write_changes(
    prepared: &colab_core::merge::Prepared,
    backup: Option<&Path>,
) -> Result<()> {
    let mut span = colab_core::telemetry::span("write");
    span.set("colab.files.written", prepared.writes.len());
    for write in &prepared.writes {
        if let Some(dir) = backup {
            write_backup_snapshot(dir, &write.path, &write.on_disk)?;
        }
        fs::write(&write.path, &write.text).map_err(|e| Error::io_at(&write.path, e))?;
    }
    Ok(())
}
