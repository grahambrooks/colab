//! Workspace edits from a `.colab` file: the code actions and commands that
//! plan a script's edits across the workspace through `colab_dsl::pipeline`,
//! apply them, undo them, and report progress.

use super::*;

/// The rule index a workspace code action was created for (`None` = all).
pub(crate) fn action_rule(action: &CodeAction) -> Option<usize> {
    action
        .data
        .as_ref()
        .and_then(|d| d.get("rule"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|r| usize::try_from(r).ok())
}

/// Whether a workspace code action is the preview (annotated) variant.
pub(crate) fn action_preview(action: &CodeAction) -> bool {
    action
        .data
        .as_ref()
        .and_then(|d| d.get("preview"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub(crate) fn lsp_error(err: Error) -> tower_lsp_server::jsonrpc::Error {
    let mut out = tower_lsp_server::jsonrpc::Error::internal_error();
    out.message = err.to_string().into();
    out
}

impl Backend {
    pub(crate) fn workspace_action(
        &self,
        title: String,
        uri: &Uri,
        rule: Option<usize>,
        preview: bool,
    ) -> CodeAction {
        CodeAction {
            title,
            kind: Some(CodeActionKind::REFACTOR_REWRITE),
            data: Some(
                serde_json::json!({ "uri": uri.as_str(), "rule": rule, "preview": preview }),
            ),
            ..Default::default()
        }
    }

    /// The edit for running the script (or rule `rule`) over the
    /// workspace: annotated for the editor's preview when `preview`.
    pub(crate) async fn edit_for(
        &self,
        uri: &Uri,
        text: &str,
        rule: Option<usize>,
        preview: bool,
    ) -> colab_core::Result<WorkspaceEdit> {
        Ok(edits::to_edit(&self.plan(uri, text, rule).await?, preview))
    }

    /// Run the script (or rule `rule`), have the client apply the result,
    /// and keep it for `colab: undo`.
    pub(crate) async fn apply_and_remember(
        &self,
        uri: &Uri,
        text: &str,
        rule: Option<usize>,
        label: String,
    ) -> LspResult<()> {
        let files = self.plan(uri, text, rule).await.map_err(lsp_error)?;
        if files.is_empty() {
            self.client
                .show_message(MessageType::INFO, format!("colab: {label} changes nothing"))
                .await;
            return Ok(());
        }
        let response = self
            .client
            .apply_edit(edits::to_edit(&files, false))
            .await?;
        if !response.applied {
            let why = response
                .failure_reason
                .unwrap_or_else(|| "no reason given".into());
            self.client
                .show_message(
                    MessageType::WARNING,
                    format!("colab: the editor did not apply {label}: {why}"),
                )
                .await;
            return Ok(());
        }
        let count = files.len();
        let mut undo = self.undo.lock().await;
        undo.push(edits::Applied {
            label: label.clone(),
            files,
        });
        let excess = undo.len().saturating_sub(UNDO_DEPTH);
        undo.drain(..excess);
        drop(undo);
        self.client
            .show_message(
                MessageType::INFO,
                format!("colab: {label} changed {count} file(s); `colab: undo` puts them back"),
            )
            .await;
        Ok(())
    }

    /// Put back the newest applied run, keeping edits made since that do
    /// not overlap it; a file whose later edits do is left alone, and named.
    pub(crate) async fn undo_last(&self) -> LspResult<()> {
        let Some(applied) = self.undo.lock().await.pop() else {
            self.client
                .show_message(MessageType::INFO, "colab: nothing to undo")
                .await;
            return Ok(());
        };
        let open = self.documents.lock().await.clone();
        let (edit, skipped) = edits::undo_edit(&applied, |file| {
            open.get(&file.uri)
                .cloned()
                .or_else(|| std::fs::read_to_string(&file.path).ok())
        });
        let response = self.client.apply_edit(edit).await?;
        let mut message = if response.applied {
            format!("colab: undid {}", applied.label)
        } else {
            format!(
                "colab: the editor did not apply the undo of {}: {}",
                applied.label,
                response
                    .failure_reason
                    .unwrap_or_else(|| "no reason given".into())
            )
        };
        if !skipped.is_empty() {
            let names: Vec<String> = skipped.iter().map(|p| p.display().to_string()).collect();
            let _ = write!(
                message,
                "; left alone, since they changed where the run did: {}",
                names.join(", ")
            );
        }
        self.client.show_message(MessageType::INFO, message).await;
        Ok(())
    }

    /// Run the script — or just rule `rule` — over every workspace root
    /// and return the rewrites as one `WorkspaceEdit` of line hunks.
    /// Nothing is written: the editor applies it.
    #[cfg(test)]
    pub(crate) async fn workspace_edit(
        &self,
        uri: &Uri,
        text: &str,
        rule: Option<usize>,
    ) -> colab_core::Result<WorkspaceEdit> {
        self.edit_for(uri, text, rule, false).await
    }

    /// Each file the script (or rule `rule`) would change across the
    /// workspace roots, with the rules that changed it.
    pub(crate) async fn plan(
        &self,
        uri: &Uri,
        text: &str,
        rule: Option<usize>,
    ) -> colab_core::Result<Vec<edits::Planned>> {
        let base = uri
            .to_file_path()
            .map_or_else(|| PathBuf::from("script.colab"), Cow::into_owned);
        let mut refactoring = compile_at_source(text, &base, &self.backends)?;
        if let Some(index) = rule {
            // `rule` is a block; a block can lower to more than one rule
            // (`and fences`), so keep that block's range of rules.
            let checks = colab_dsl::check(text, &self.backends)?;
            let count = |c: &colab_dsl::RuleCheck| c.outcome.as_ref().map_or(0, Vec::len);
            let Some(block) = checks.get(index) else {
                return Err(Error::Config(format!("the script has no rule {index}")));
            };
            let start: usize = checks[..index].iter().map(count).sum();
            let end = start + count(block);
            if end > refactoring.rules.len() {
                return Err(Error::Config(format!("the script has no rule {index}")));
            }
            refactoring.rules = refactoring.rules.drain(start..end).collect();
        }
        let mut roots = self.roots.lock().await.clone();
        if roots.is_empty()
            && let Some(parent) = base.parent()
        {
            roots.push(parent.to_path_buf());
        }
        let progress = self.begin_progress(&refactoring.name).await;
        // The walk is blocking, CPU-bound work on a runtime worker; progress
        // reports go out from it through the runtime handle.
        let handle = tokio::runtime::Handle::current();
        // Progress per file the walk produces, sent through the runtime.
        pub(crate) struct Ticks<F: FnMut(usize, usize)> {
            seen: usize,
            changing: usize,
            tick: F,
        }
        impl<F: FnMut(usize, usize)> colab_dsl::pipeline::Hooks for Ticks<F> {
            fn filter(&mut self, change: colab_core::FileChange) -> colab_core::FileChange {
                self.seen += 1;
                self.changing += usize::from(change.changed());
                if self.seen.is_multiple_of(PROGRESS_EVERY) {
                    (self.tick)(self.seen, self.changing);
                }
                change
            }
        }
        let mut ticks = Ticks {
            seen: 0,
            changing: 0,
            tick: |seen: usize, changing: usize| {
                if let Some(token) = &progress {
                    handle.block_on(self.report_progress(
                        token,
                        format!("{seen} file(s) scanned, {changing} to change"),
                    ));
                }
            },
        };
        // A run of its own: requests on other runtime workers never share
        // its oracle, facts or failures. The pipeline gives it what every
        // run has: scan phases, the semantic gate, and conflicts checked
        // before the client is handed an edit.
        let run = colab_core::run::Run::new();
        let labels: Vec<String> = refactoring.rules.iter().map(ToString::to_string).collect();
        let changes = tokio::task::block_in_place(|| {
            run.enter(|| {
                // A trace per walk, opened and closed on this thread: an
                // async task can move between threads at an `.await`.
                let mut span = colab_core::telemetry::root("colab server walk");
                span.set("colab.server.rule", rule.is_some());
                let source = colab_dsl::pipeline::Source::Walk {
                    targets: roots.clone(),
                    options: WalkOptions::default(),
                };
                let outcome =
                    colab_dsl::pipeline::Pipeline::new(&refactoring, source).execute(&mut ticks)?;
                if let Some(first) = outcome.report.conflicts().first() {
                    return Err(Error::Config(format!(
                        "made no edit: {} conflict(s) found, the first in {}: {}",
                        outcome.report.conflicts().len(),
                        first.path.display(),
                        first.message
                    )));
                }
                Ok::<Vec<edits::Planned>, Error>(
                    outcome
                        .changes
                        .into_iter()
                        .filter_map(|change| {
                            Some(edits::Planned {
                                uri: Uri::from_file_path(&change.path)?,
                                rules: change
                                    .rules_fired
                                    .iter()
                                    .filter_map(|&i| labels.get(i).cloned())
                                    .collect(),
                                path: change.path,
                                before: change.before,
                                after: change.after,
                            })
                        })
                        .collect(),
                )
            })
        });
        let seen = ticks.seen;
        if let Some(token) = progress {
            let message = match &changes {
                Ok(c) => format!("{} file(s) to change, of {seen} scanned", c.len()),
                Err(e) => format!("failed: {e}"),
            };
            self.end_progress(token, message).await;
        }
        changes
    }

    /// Start a progress report for a workspace edit, if the client shows
    /// server-initiated progress; `None` otherwise (or if it declines).
    pub(crate) async fn begin_progress(&self, script: &str) -> Option<NumberOrString> {
        if !self.progress.load(Ordering::Relaxed) {
            return None;
        }
        let n = PROGRESS_TOKENS.fetch_add(1, Ordering::Relaxed);
        let token = NumberOrString::String(format!("colab-apply-{n}"));
        self.client
            .send_request::<request::WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                token: token.clone(),
            })
            .await
            .ok()?;
        self.client
            .send_notification::<notification::Progress>(ProgressParams {
                token: token.clone(),
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                    WorkDoneProgressBegin {
                        title: format!("colab: `{script}`"),
                        cancellable: Some(false),
                        message: Some("scanning the workspace".into()),
                        percentage: None,
                    },
                )),
            })
            .await;
        Some(token)
    }

    pub(crate) async fn report_progress(&self, token: &NumberOrString, message: String) {
        self.client
            .send_notification::<notification::Progress>(ProgressParams {
                token: token.clone(),
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::Report(
                    WorkDoneProgressReport {
                        cancellable: Some(false),
                        message: Some(message),
                        percentage: None,
                    },
                )),
            })
            .await;
    }

    pub(crate) async fn end_progress(&self, token: NumberOrString, message: String) {
        self.client
            .send_notification::<notification::Progress>(ProgressParams {
                token,
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(WorkDoneProgressEnd {
                    message: Some(message),
                })),
            })
            .await;
    }
}
