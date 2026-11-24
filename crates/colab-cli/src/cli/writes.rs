//! What the CLI plugs into `colab_dsl::pipeline`: the checks a run's
//! flags ask for (`--tidy`, the Markdown link check, `--verify-lsp`), and
//! the hooks that stream files to the reporter, apply `--baseline` and save
//! `--backup` snapshots.

use super::*;

use codemod::pipeline::{Check, Hooks};

/// Exit code for a `--write` run that found conflicts and wrote nothing.
pub(super) const CONFLICT_EXIT: u8 = 11;

/// How a CLI user overrides a refused write.
pub(super) const REFUSAL_HINT: &str =
    "review them, then fix the rules or rerun with --allow-conflicts";

/// `--tidy`: format the changed files before they are recorded.
pub(super) struct Tidy {
    command: Option<String>,
    formatters: std::collections::BTreeMap<String, String>,
}

impl Check for Tidy {
    fn rewrite(&mut self, changes: &mut Vec<FileChange>, report: &mut RunReport) -> Result<()> {
        crate::lsp_commands::tidy_pending(
            self.command.as_deref(),
            &self.formatters,
            changes,
            report,
        )
    }
}

/// The Markdown link check: a link the run would break is a conflict.
pub(crate) struct LinkCheck {
    pub(crate) roots: Vec<PathBuf>,
}

impl Check for LinkCheck {
    fn verify(
        &mut self,
        _rules: &[Box<dyn colab_core::Operation>],
        changes: &[FileChange],
        report: &mut RunReport,
    ) -> Result<()> {
        crate::markdown::check_links(changes, &self.roots, report)
    }
}

/// `--verify-lsp`: a new error diagnostic is a conflict.
pub(super) struct VerifyLsp {
    command: Option<String>,
}

impl Check for VerifyLsp {
    fn verify(
        &mut self,
        rules: &[Box<dyn colab_core::Operation>],
        changes: &[FileChange],
        report: &mut RunReport,
    ) -> Result<()> {
        crate::lsp_commands::verify_lsp(self.command.as_deref(), changes, report, Some(rules))
    }
}

/// The checks `args` asks for, in the order they run: `--tidy`, then the
/// Markdown link check (unless `--no-link-check`), then `--verify-lsp`.
pub(super) fn checks<'a>(args: &RefactorArgs) -> Result<Vec<Box<dyn Check + 'a>>> {
    let mut checks: Vec<Box<dyn Check + 'a>> = Vec::new();
    if let Some(command) = &args.tidy {
        checks.push(Box::new(Tidy {
            command: Some(command.clone()).filter(|c| !c.is_empty()),
            formatters: formatters()?,
        }));
    }
    if !args.no_link_check {
        let roots = if args.paths.is_empty() {
            vec![PathBuf::from(".")]
        } else {
            args.paths.clone()
        };
        checks.push(Box::new(LinkCheck { roots }));
    }
    if let Some(command) = &args.verify_lsp {
        checks.push(Box::new(VerifyLsp {
            command: Some(command.clone()).filter(|c| !c.is_empty()),
        }));
    }
    Ok(checks)
}

/// The CLI's view of a run: unchanged files go to the reporter as the walk
/// finds them, `--baseline` filters each file, and `--backup` saves each
/// file before it is written.
pub(super) struct CliHooks<'a> {
    pub(super) reporter: &'a mut dyn format::Reporter,
    pub(super) baseline: Option<(&'a mut crate::baseline::Baseline, &'a RunReport)>,
    pub(super) backup: Option<&'a Path>,
}

impl<'a> CliHooks<'a> {
    pub(super) fn new(reporter: &'a mut dyn format::Reporter, backup: Option<&'a Path>) -> Self {
        Self {
            reporter,
            baseline: None,
            backup,
        }
    }
}

impl Hooks for CliHooks<'_> {
    fn filter(&mut self, change: FileChange) -> FileChange {
        match self.baseline.as_mut() {
            Some((baseline, rules_only)) => baseline.filter(change, rules_only),
            None => change,
        }
    }

    fn unchanged(&mut self, change: &FileChange) -> Result<()> {
        self.reporter
            .report(change)
            .map_err(|e| Error::io_at(&change.path, e))
    }

    fn walked(&mut self, report: &mut RunReport) {
        if let Some((baseline, _)) = &self.baseline
            && baseline.dropped > 0
        {
            report.warn(format!(
                "{} finding(s) left out: they are already at {} (--baseline)",
                baseline.dropped, baseline.reference
            ));
        }
    }

    fn before_write(&mut self, path: &Path, on_disk: &str) -> Result<()> {
        match self.backup {
            Some(dir) => write_backup_snapshot(dir, path, on_disk),
            None => Ok(()),
        }
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
