//! The language-server oracle a script's `type` / `resolves` constraints
//! ask, installed for the run. (Scripts with `scan` phases need nothing
//! here: every run path reads them through `colab_dsl::pipeline`.)

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
