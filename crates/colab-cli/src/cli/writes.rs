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

/// Write each change, saving its original under `backup` first.
pub(super) fn write_changes(changes: &[FileChange], backup: Option<&Path>) -> Result<()> {
    for change in changes {
        if let Some(dir) = backup {
            write_backup_snapshot(dir, &change.path, &change.before)?;
        }
        fs::write(&change.path, &change.after).map_err(|e| Error::io_at(&change.path, e))?;
    }
    Ok(())
}
