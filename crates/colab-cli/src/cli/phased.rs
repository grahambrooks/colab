//! The pieces a run needs before it walks: the language-server oracle a
//! script's `type` / `resolves` constraints ask, and phased runs (scripts
//! with `scan` phases), which read every file before any rule changes one.

use super::*;

/// The language-server oracle for a run whose rules ask one (`type`,
/// `resolves`), installed while it lives.
pub(super) struct OracleGuard(Option<std::sync::Arc<colab_lsp_client::semantic::LspOracle>>);

impl OracleGuard {
    pub(super) fn install(
        refactoring: &codemod::Refactoring,
        lsp: Option<&str>,
        timeout: u64,
    ) -> Result<Self> {
        if !refactoring.needs_oracle() {
            return Ok(Self(None));
        }
        let server = lsp
            .map(colab_lsp_client::ServerCommand::parse)
            .transpose()?;
        let root = std::env::current_dir().map_err(io_to_error)?;
        let oracle = std::sync::Arc::new(colab_lsp_client::semantic::LspOracle::new(
            &root,
            server,
            std::time::Duration::from_secs(timeout),
        ));
        colab_core::semantic::install(oracle.clone());
        Ok(Self(Some(oracle)))
    }
}

impl Drop for OracleGuard {
    fn drop(&mut self) {
        if let Some(oracle) = self.0.take() {
            colab_core::semantic::uninstall();
            colab_core::semantic::take_failure();
            oracle.shutdown();
        }
    }
}

/// A question a rule's `type` / `resolves` constraint could not have
/// answered fails the run, before anything is written.
pub(super) fn semantic_failure() -> Result<()> {
    match colab_core::semantic::take_failure().or_else(colab_core::facts::take_failure) {
        Some(why) => Err(Error::Config(why)),
        None => Ok(()),
    }
}

/// Visits every file a phased run needs (one any rule or scan applies
/// to), changing nothing: what [`phased_walk`] reads into memory.
pub(super) struct Relevant<'a>(pub(super) &'a codemod::Refactoring);

impl CodeTransformer for Relevant<'_> {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.0.is_file_relevant(path)
            || self
                .0
                .stages
                .iter()
                .flat_map(|s| &s.scans)
                .any(|scan| scan.op.is_file_relevant(path))
    }

    fn apply(&self, source: &str) -> String {
        source.to_string()
    }
}

/// A script with `scan` phases: every file it needs read into memory, the
/// stages run over all of them (`Refactoring::run_phased`), and each
/// file's result visited as an ordinary walk would.
pub(super) fn phased_walk<F>(
    refactoring: &codemod::Refactoring,
    args: &RefactorArgs,
    targets: &[PathBuf],
    visit: &mut F,
) -> Result<WalkOutcome>
where
    F: FnMut(FileChange) -> Result<()>,
{
    let mut files = Vec::new();
    let outcome = walk_targets(
        &Relevant(refactoring),
        args,
        targets,
        &mut |change: FileChange| {
            files.push((change.path, change.before));
            Ok(())
        },
    )?;
    for change in refactoring.run_phased(files) {
        visit(change)?;
    }
    semantic_failure()?;
    Ok(outcome)
}

/// The run paths a script with scans cannot take yet: each walks the
/// files once, one rule at a time, or reads one file.
pub(super) fn refuse_phased(refactoring: &codemod::Refactoring, args: &RefactorArgs) -> Result<()> {
    if !refactoring.has_scans() {
        return Ok(());
    }
    let flag = [
        (args.split_pr.is_some(), "--split-pr"),
        (args.verify.is_some(), "--verify"),
        (args.commit_per_rule, "--commit-per-rule"),
        (args.bisect.is_some(), "--bisect"),
        (args.changed_since.is_some(), "--changed-since"),
        (args.staged, "--staged"),
        (args.stdin, "--stdin"),
    ]
    .into_iter()
    .find_map(|(on, name)| on.then_some(name));
    match flag {
        Some(flag) => Err(Error::UnsupportedOperation(format!(
            "a script with `scan` phases reads every file before it edits one, which {flag} does not do yet; run it without {flag}"
        ))),
        None => Ok(()),
    }
}
