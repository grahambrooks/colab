//! The pieces a run needs before it walks: the language-server oracle a
//! script's `type` / `resolves` constraints ask, and the run paths that
//! refuse phased runs (scripts with `scan` phases, which
//! `colab_dsl::pipeline` reads whole before any rule changes a file).

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

/// The run paths a script with scans cannot take yet: each applies one
/// rule at a time, or reads only some of the files (a scan must see them
/// all), or reads one text.
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
