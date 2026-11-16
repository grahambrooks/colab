//! LSP server for `.colab` scripts.
//!
//! Today serves two features:
//!
//! - **Diagnostics.** Every open / change of a `.colab` file
//!   triggers `colab_dsl::compile` against the binary's default
//!   [`BackendRegistry`]; parse errors and unsupported-namespace
//!   errors surface as LSP diagnostics. Parse errors carry a
//!   [`colab_core::ParseDetail`] and so are placed on the offending
//!   line and column; errors without a position (an unsupported
//!   namespace, say) fall back to the start of the file.
//! - **Completion.** Suggests namespaces (`go::`, `rust::`, …),
//!   modules (per backend), and actions (`replace`/`delete`/`ensure`/
//!   `replace_call`) based on cursor context. Items are sourced from
//!   `BackendRegistry::capabilities` so completion stays in lockstep
//!   with `colab list-rules`.
//!
//! - **Hover.** On `<lang>::<module>`, the module's description and its
//!   actions; on an action keyword, what that action does *for the
//!   enclosing rule's module*.
//! - **Workspace code actions.** On any rule: "apply this rule to the
//!   workspace", and "apply the whole script". The result is an ordinary
//!   `WorkspaceEdit`, which editors present as a reviewable refactor
//!   preview; it is computed on `codeAction/resolve`, so moving the cursor
//!   costs nothing.
//!
//! Diagnostics are per rule: [`colab_dsl::check`] lowers each block on its
//! own, so every bad rule — an unknown module, a `pattern` that is not
//! valid code — is underlined where it is written.
use colab_lsp_client::edit::lsp_u32;
use std::fmt::Write as _;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex as AsyncMutex;
use tower_lsp_server::jsonrpc::Result as LspResult;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

use colab_core::walker::{self, WalkOptions};
use colab_core::{BackendRegistry, Error};
use colab_dsl::{RuleCheck, compile, compile_at_source};

/// Place a diagnostic at the position the error carries.
///
/// LSP positions are 0-based; [`colab_core::ParseDetail`] is 1-based.
/// The range covers a single character at the offending token — enough
/// for an editor to put the squiggle in the right place. Errors with no
/// position (an unsupported namespace) land at the start of the file.
fn error_range(err: &Error) -> Range {
    match err {
        Error::Parse(detail) => {
            let line = lsp_u32(detail.line.saturating_sub(1));
            let character = lsp_u32(detail.column.saturating_sub(1));
            Range::new(
                Position::new(line, character),
                Position::new(line, character + 1),
            )
        }
        _ => Range::new(Position::new(0, 0), Position::new(0, 0)),
    }
}

mod edits;

/// The commands `colab server` executes (`workspace/executeCommand`).
const APPLY_COMMAND: &str = "colab.apply";
const UNDO_COMMAND: &str = "colab.undo";
/// How many applied runs `colab: undo` can step back through.
const UNDO_DEPTH: usize = 10;

const STATIC_ACTIONS: &[(&str, &str)] = &[
    ("replace", "replace \"$0\""),
    ("delete", "delete"),
    ("ensure", "ensure"),
    ("replace_call", "replace_call \"$0\""),
    ("set", "set '$0'"),
    ("insert", "insert '$0'"),
];

/// Tower-LSP backend. Holds the open-document store and a backend
/// registry the diagnostics + completion handlers consult.
struct Backend {
    client: Client,
    documents: AsyncMutex<HashMap<Uri, String>>,
    backends: BackendRegistry,
    /// Workspace roots from `initialize`; what the code actions rewrite.
    roots: AsyncMutex<Vec<PathBuf>>,
    /// The client resolves code-action edits lazily (`resolveSupport`
    /// lists `edit`). Without it the edit is computed up front.
    lazy_edits: AtomicBool,
    /// The client shows server-initiated progress
    /// (`window.workDoneProgress`), used while a workspace edit walks.
    progress: AtomicBool,
    /// The client takes annotated document changes
    /// (`workspace.workspaceEdit.changeAnnotationSupport`), so a preview
    /// action can ask before applying, hunk by hunk.
    annotations: AtomicBool,
    /// The client applies server-sent edits (`workspace.applyEdit`), so
    /// `colab: apply` runs as a command and can be undone.
    apply_edit: AtomicBool,
    /// Applied runs, newest last, for `colab: undo`.
    undo: AsyncMutex<Vec<edits::Applied>>,
}

/// A progress report per this many files walked.
const PROGRESS_EVERY: usize = 25;

/// Distinct tokens for concurrent workspace edits.
static PROGRESS_TOKENS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The path of the `include "<path>"` (any quoting the DSL takes) whose
/// keyword or string holds `offset`, if any.
fn include_at(text: &str, offset: usize) -> Option<String> {
    let mut from = 0;
    while let Some(at) = text[from..].find("include") {
        let start = from + at;
        from = start + "include".len();
        // A whole word, at the start of an item.
        if text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':')
        {
            continue;
        }
        let rest = &text[from..];
        let gap = rest.len() - rest.trim_start().len();
        let quoted = &rest[gap..];
        let (open, close) = if quoted.starts_with("r##\"") {
            ("r##\"", "\"##")
        } else if quoted.starts_with("r#\"") {
            ("r#\"", "\"#")
        } else if quoted.starts_with('"') {
            ("\"", "\"")
        } else if quoted.starts_with('\'') {
            ("'", "'")
        } else {
            continue;
        };
        let body = &quoted[open.len()..];
        let Some(len) = body.find(close) else {
            continue;
        };
        let end = from + gap + open.len() + len + close.len();
        if (start..=end).contains(&offset) {
            return Some(body[..len].to_string());
        }
    }
    None
}

fn diagnostic(range: Range, err: &Error) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::Number(i32::from(err.exit_code()))),
        source: Some("colab".into()),
        message: format!("{err}"),
        related_information: None,
        tags: None,
        code_description: None,
        data: None,
    }
}

impl Backend {
    fn new(client: Client, backends: BackendRegistry) -> Self {
        Self {
            client,
            documents: AsyncMutex::new(HashMap::new()),
            backends,
            roots: AsyncMutex::new(Vec::new()),
            lazy_edits: AtomicBool::new(false),
            progress: AtomicBool::new(false),
            annotations: AtomicBool::new(false),
            apply_edit: AtomicBool::new(false),
            undo: AsyncMutex::new(Vec::new()),
        }
    }

    /// Diagnose a script with no known location (`include` unresolvable).
    #[cfg(test)]
    fn diagnose(&self, text: &str) -> Vec<Diagnostic> {
        self.diagnose_at(None, text)
    }

    /// Every problem in the script, each placed on the rule that caused
    /// it. An empty list means the script compiles cleanly.
    ///
    /// A syntax error is reported alone, at its token. Otherwise each
    /// `match` block is lowered independently, so three bad rules give
    /// three diagnostics. Finally, if every local rule is fine, the whole
    /// script is compiled — relative to `path`, when the document has one
    /// — to catch `include` problems, which land on the first `include`.
    fn diagnose_at(&self, path: Option<&Path>, text: &str) -> Vec<Diagnostic> {
        let checks = match colab_dsl::check(text, &self.backends) {
            Ok(checks) => checks,
            Err(err) => return vec![diagnostic(error_range(&err), &err)],
        };
        let mut out: Vec<Diagnostic> = checks
            .iter()
            .filter_map(|check| {
                let err = check.outcome.as_ref().err()?;
                Some(diagnostic(span_range(text, check), err))
            })
            .collect();
        // Advice (a rewrite that matches its own output) is a warning: the
        // rule compiles, but probably does not do what was meant.
        out.extend(checks.iter().flat_map(|check| {
            check.advice.iter().map(|advice| Diagnostic {
                range: span_range(text, check),
                severity: Some(DiagnosticSeverity::WARNING),
                source: Some("colab".into()),
                message: advice.clone(),
                ..Default::default()
            })
        }));
        if out.is_empty() && text.contains("include") {
            let whole = match path {
                Some(path) => compile_at_source(text, path, &self.backends),
                None => compile(text, &self.backends),
            };
            if let Err(err) = whole {
                let range = text
                    .find("include")
                    .map(|at| {
                        Range::new(
                            position_at(text, at),
                            position_at(text, at + "include".len()),
                        )
                    })
                    .unwrap_or_default();
                out.push(diagnostic(range, &err));
            }
        }
        out
    }

    async fn refresh_diagnostics(&self, uri: Uri, version: Option<i32>, text: String) {
        // Only diagnose `.colab` files; ignore everything else so
        // editors that route arbitrary buffers here don't pollute
        // the diagnostics panel.
        if !is_codemod_uri(&uri) {
            self.documents.lock().await.insert(uri, text);
            return;
        }
        let path = uri.to_file_path();
        let diagnostics = self.diagnose_at(path.as_deref(), &text);
        self.documents.lock().await.insert(uri.clone(), text);
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }
}

fn is_codemod_uri(uri: &Uri) -> bool {
    colab_core::script_file::is_script(Path::new(uri.path().as_str()))
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
        let mut roots: Vec<PathBuf> = params
            .workspace_folders
            .unwrap_or_default()
            .iter()
            .filter_map(|f| f.uri.to_file_path().map(Cow::into_owned))
            .collect();
        #[allow(deprecated)] // `root_uri` is still what many clients send
        if roots.is_empty()
            && let Some(root) = params
                .root_uri
                .and_then(|u| u.to_file_path().map(Cow::into_owned))
        {
            roots.push(root);
        }
        *self.roots.lock().await = roots;
        let lazy = params
            .capabilities
            .text_document
            .and_then(|t| t.code_action)
            .and_then(|c| c.resolve_support)
            .is_some_and(|r| r.properties.iter().any(|p| p == "edit"));
        self.lazy_edits.store(lazy, Ordering::Relaxed);
        let progress = params
            .capabilities
            .window
            .as_ref()
            .and_then(|w| w.work_done_progress)
            .unwrap_or(false);
        self.progress.store(progress, Ordering::Relaxed);
        let workspace = params.capabilities.workspace.as_ref();
        let annotations = workspace
            .and_then(|w| w.workspace_edit.as_ref())
            .is_some_and(|e| {
                e.document_changes == Some(true) && e.change_annotation_support.is_some()
            });
        self.annotations.store(annotations, Ordering::Relaxed);
        let apply_edit = workspace.and_then(|w| w.apply_edit) == Some(true);
        self.apply_edit.store(apply_edit, Ordering::Relaxed);

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "colab-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![":".into(), " ".into()]),
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![CodeActionKind::REFACTOR_REWRITE]),
                        resolve_provider: Some(true),
                        ..Default::default()
                    },
                )),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![APPLY_COMMAND.into(), UNDO_COMMAND.into()],
                    ..Default::default()
                }),
                ..Default::default()
            },
            // clangd's UTF-8 offsets extension; positions here are UTF-16.
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(
                MessageType::INFO,
                format!("colab LSP ready; backends: {:?}", self.backends.languages()),
            )
            .await;
    }

    async fn shutdown(&self) -> LspResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let TextDocumentItem {
            uri, text, version, ..
        } = params.text_document;
        self.refresh_diagnostics(uri, Some(version), text).await;
    }

    async fn did_change(&self, mut params: DidChangeTextDocumentParams) {
        // text_document_sync == FULL means each change carries the
        // full document in the first content_changes entry.
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let Some(change) = params.content_changes.pop() else {
            return;
        };
        self.refresh_diagnostics(uri, Some(version), change.text)
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.lock().await.remove(&uri);
        // Clear stale diagnostics so editors don't keep showing them
        // for a file no longer open.
        if is_codemod_uri(&uri) {
            self.client.publish_diagnostics(uri, Vec::new(), None).await;
        }
    }

    async fn completion(&self, params: CompletionParams) -> LspResult<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        if !is_codemod_uri(&uri) {
            return Ok(None);
        }
        let docs = self.documents.lock().await;
        let Some(text) = docs.get(&uri) else {
            return Ok(None);
        };
        let prefix = line_prefix(text, position);
        Ok(Some(CompletionResponse::Array(
            self.completions_for(prefix),
        )))
    }

    /// `include "<path>"` under the cursor goes to that script, resolved
    /// as the compiler resolves it: against the including file's directory.
    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> LspResult<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        if !is_codemod_uri(&uri) {
            return Ok(None);
        }
        let docs = self.documents.lock().await;
        let Some(text) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = offset_at(text, position.position);
        let (Some(target), Some(here)) = (include_at(text, offset), uri.to_file_path()) else {
            return Ok(None);
        };
        let path = here.parent().unwrap_or(Path::new(".")).join(target);
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Uri::from_file_path(&path)
            .map(|target| GotoDefinitionResponse::Scalar(Location::new(target, Range::default()))))
    }

    async fn hover(&self, params: HoverParams) -> LspResult<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        if !is_codemod_uri(&uri) {
            return Ok(None);
        }
        let docs = self.documents.lock().await;
        let Some(text) = docs.get(&uri) else {
            return Ok(None);
        };
        let offset = offset_at(text, params.text_document_position_params.position);
        Ok(self.hover_at(text, offset).map(|markdown| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: markdown,
            }),
            range: None,
        }))
    }

    async fn code_action(&self, params: CodeActionParams) -> LspResult<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        if !is_codemod_uri(&uri) {
            return Ok(None);
        }
        let text = match self.documents.lock().await.get(&uri) {
            Some(text) => text.clone(),
            None => return Ok(None),
        };
        let Ok(checks) = colab_dsl::check(&text, &self.backends) else {
            return Ok(None);
        };
        // Only offer to run a script that compiles: a broken one has
        // diagnostics to fix first.
        if checks.iter().any(|c| c.outcome.is_err()) || checks.is_empty() {
            return Ok(None);
        }
        let offset = offset_at(&text, params.range.start);
        // What to offer the actions for: the rule under the cursor, then
        // the whole script.
        let mut targets: Vec<(String, Option<usize>)> = Vec::new();
        if let Some((index, check)) = checks
            .iter()
            .enumerate()
            .find(|(_, c)| c.span.start <= offset && offset <= c.span.end)
        {
            let description = check
                .outcome
                .as_ref()
                .ok()
                .and_then(|rules| rules.first())
                .map_or("", String::as_str);
            targets.push((format!("`{description}`"), Some(index)));
        }
        targets.push(("the whole script".into(), None));
        let preview = self.annotations.load(Ordering::Relaxed);
        let apply_edit = self.apply_edit.load(Ordering::Relaxed);
        let mut actions = Vec::new();
        for (what, rule) in &targets {
            if preview {
                actions.push(self.workspace_action(
                    format!("colab: preview {what} across the workspace"),
                    &uri,
                    *rule,
                    true,
                ));
            }
            let title = format!("colab: apply {what} to the workspace");
            actions.push(if apply_edit {
                // A command, so the server sees the edit applied and can
                // offer to undo it.
                CodeAction {
                    title: title.clone(),
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    command: Some(Command::new(
                        title,
                        APPLY_COMMAND.into(),
                        Some(vec![serde_json::json!({
                            "uri": uri.as_str(),
                            "rule": rule,
                            "label": what,
                        })]),
                    )),
                    ..Default::default()
                }
            } else {
                self.workspace_action(title, &uri, *rule, false)
            });
        }
        if apply_edit && let Some(last) = self.undo.lock().await.last() {
            let title = format!("colab: undo applying {}", last.label);
            actions.push(CodeAction {
                title: title.clone(),
                kind: Some(CodeActionKind::REFACTOR_REWRITE),
                command: Some(Command::new(title, UNDO_COMMAND.into(), None)),
                ..Default::default()
            });
        }
        if !self.lazy_edits.load(Ordering::Relaxed) {
            for action in actions.iter_mut().filter(|a| a.data.is_some()) {
                let (rule, preview) = (action_rule(action), action_preview(action));
                action.edit = Some(
                    self.edit_for(&uri, &text, rule, preview)
                        .await
                        .map_err(lsp_error)?,
                );
                action.data = None;
            }
        }
        Ok(Some(
            actions
                .into_iter()
                .map(CodeActionOrCommand::CodeAction)
                .collect(),
        ))
    }

    async fn code_action_resolve(&self, mut action: CodeAction) -> LspResult<CodeAction> {
        let Some(data) = action.data.clone() else {
            return Ok(action);
        };
        let Some(uri) = data
            .get("uri")
            .and_then(|u| u.as_str())
            .and_then(|u| u.parse::<Uri>().ok())
        else {
            return Ok(action);
        };
        let Some(text) = self.documents.lock().await.get(&uri).cloned() else {
            return Ok(action);
        };
        let (rule, preview) = (action_rule(&action), action_preview(&action));
        action.edit = Some(
            self.edit_for(&uri, &text, rule, preview)
                .await
                .map_err(lsp_error)?,
        );
        Ok(action)
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> LspResult<Option<serde_json::Value>> {
        match params.command.as_str() {
            APPLY_COMMAND => {
                let arg = params.arguments.first().cloned().unwrap_or_default();
                let Some(uri) = arg["uri"].as_str().and_then(|u| u.parse::<Uri>().ok()) else {
                    return Err(tower_lsp_server::jsonrpc::Error::invalid_params(
                        "colab.apply takes {uri, rule?}",
                    ));
                };
                let rule = arg["rule"].as_u64().and_then(|r| usize::try_from(r).ok());
                let label = arg["label"].as_str().unwrap_or("the script").to_string();
                let Some(text) = self.documents.lock().await.get(&uri).cloned() else {
                    return Ok(None);
                };
                self.apply_and_remember(&uri, &text, rule, label).await?;
                Ok(None)
            }
            UNDO_COMMAND => {
                self.undo_last().await?;
                Ok(None)
            }
            other => Err(tower_lsp_server::jsonrpc::Error::invalid_params(format!(
                "unknown command `{other}`; colab server runs {APPLY_COMMAND} and {UNDO_COMMAND}"
            ))),
        }
    }
}

/// The rule index a workspace code action was created for (`None` = all).
fn action_rule(action: &CodeAction) -> Option<usize> {
    action
        .data
        .as_ref()
        .and_then(|d| d.get("rule"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|r| usize::try_from(r).ok())
}

/// Whether a workspace code action is the preview (annotated) variant.
fn action_preview(action: &CodeAction) -> bool {
    action
        .data
        .as_ref()
        .and_then(|d| d.get("preview"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn lsp_error(err: Error) -> tower_lsp_server::jsonrpc::Error {
    let mut out = tower_lsp_server::jsonrpc::Error::internal_error();
    out.message = err.to_string().into();
    out
}

impl Backend {
    fn workspace_action(
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
    async fn edit_for(
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
    async fn apply_and_remember(
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
    async fn undo_last(&self) -> LspResult<()> {
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
    async fn workspace_edit(
        &self,
        uri: &Uri,
        text: &str,
        rule: Option<usize>,
    ) -> colab_core::Result<WorkspaceEdit> {
        self.edit_for(uri, text, rule, false).await
    }

    /// Each file the script (or rule `rule`) would change across the
    /// workspace roots, with the rules that changed it.
    async fn plan(
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
        let mut seen = 0usize;
        let mut changing = 0usize;
        // A run of its own: requests on other runtime workers never share
        // its oracle, facts or failures.
        let run = colab_core::run::Run::new();
        let labels: Vec<String> = refactoring.rules.iter().map(ToString::to_string).collect();
        let changes = tokio::task::block_in_place(|| {
            run.enter(|| {
                // A trace per walk, opened and closed on this thread: an
                // async task can move between threads at an `.await`.
                let mut span = colab_core::telemetry::root("colab server walk");
                span.set("colab.server.rule", rule.is_some());
                let mut changes: Vec<edits::Planned> = Vec::new();
                for root in &roots {
                    walker::walk_with(
                        &refactoring,
                        root,
                        &WalkOptions::default(),
                        &mut |change| {
                            seen += 1;
                            changing += usize::from(change.changed());
                            if let Some(token) = &progress
                                && seen.is_multiple_of(PROGRESS_EVERY)
                            {
                                handle.block_on(self.report_progress(
                                    token,
                                    format!("{seen} file(s) scanned, {changing} to change"),
                                ));
                            }
                            if change.changed()
                                && let Some(uri) = Uri::from_file_path(&change.path)
                            {
                                changes.push(edits::Planned {
                                    uri,
                                    rules: change
                                        .rules_fired
                                        .iter()
                                        .filter_map(|&i| labels.get(i).cloned())
                                        .collect(),
                                    path: change.path,
                                    before: change.before,
                                    after: change.after,
                                });
                            }
                            Ok(())
                        },
                    )?;
                }
                Ok::<_, Error>(changes)
            })
        });
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
    async fn begin_progress(&self, script: &str) -> Option<NumberOrString> {
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

    async fn report_progress(&self, token: &NumberOrString, message: String) {
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

    async fn end_progress(&self, token: NumberOrString, message: String) {
        self.client
            .send_notification::<notification::Progress>(ProgressParams {
                token,
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(WorkDoneProgressEnd {
                    message: Some(message),
                })),
            })
            .await;
    }

    /// Markdown for the token at byte `offset`, if colab knows it.
    fn hover_at(&self, text: &str, offset: usize) -> Option<String> {
        let word = word_at(text, offset)?;
        if let Some((lang, module)) = word.split_once("::") {
            let backend = self.backends.get(lang)?;
            let cap = backend.capabilities().iter().find(|c| c.module == module)?;
            let mut out = format!("**`{lang}::{module}`** — {}\n\nActions:\n", cap.description);
            for action in cap.actions {
                let _ = writeln!(out, "- `{}` — {}", action.name, action.description);
            }
            return Some(out);
        }
        if STATIC_ACTIONS.iter().any(|(name, _)| *name == word) {
            let checks = colab_dsl::check(text, &self.backends).ok()?;
            let check = checks
                .iter()
                .find(|c| c.span.start <= offset && offset <= c.span.end)?;
            let (lang, module) = check.namespace.split_once("::")?;
            let cap = self
                .backends
                .get(lang)?
                .capabilities()
                .iter()
                .find(|c| c.module == module)?;
            return match cap.actions.iter().find(|a| a.name == word) {
                Some(action) => Some(format!(
                    "**`{word}`** on `{}` — {}",
                    check.namespace, action.description
                )),
                None => Some(format!(
                    "`{}` has no `{word}` action; it supports {}.",
                    check.namespace,
                    cap.actions
                        .iter()
                        .map(|a| format!("`{}`", a.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            };
        }
        let keyword = match word {
            "refactor" => {
                "**`refactor \"<name>\" { … }`** — one named script; its `match` blocks run in order."
            }
            "match" => {
                "**`match <lang>::<module> \"<target>\" [in \"<glob>\"] { <action> }`** — one rule."
            }
            "in" => {
                "**`in \"<glob>\"`** — limit the rule to matching paths (a leading `./` is ignored)."
            }
            "include" => {
                "**`include \"<path>\"`** — splice another script's rules in here, resolved relative to this file."
            }
            "pattern" => {
                "**`pattern <name> = '<code>'`** — a pattern `where` clauses name: `inside <name>`, `has`, `follows`, `precedes`, `until`. Script-wide, shared through `include`."
            }
            "let" => {
                "**`let $NEW = $FROM | step | …`** — before a `<lang>::pattern` rule's `replace`: a value derived from a binding, for the template. Steps: `lower`, `upper`, `capitalize`, `snake`, `kebab`, `camel`, `pascal`, `upper_snake`, `strip_prefix(\"…\")`, `strip_suffix(\"…\")`, `replace(/re/, \"with\")`, `substring(start, end)`."
            }
            _ => return None,
        };
        Some(keyword.to_string())
    }
}

/// The `[A-Za-z0-9_:]` run around byte `offset`, if any.
fn word_at(text: &str, offset: usize) -> Option<&str> {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == ':';
    let offset = offset.min(text.len());
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(i, _)| offset + i);
    let word = text[start..end].trim_matches(':');
    (!word.is_empty()).then_some(word)
}

/// LSP position (0-based line, UTF-16 column) of byte `offset`.
fn position_at(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = lsp_u32(before.matches('\n').count());
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let character = lsp_u32(text[line_start..offset].encode_utf16().count());
    Position::new(line, character)
}

/// Byte offset of an LSP position (UTF-16 column), clamped to the text.
fn offset_at(text: &str, position: Position) -> usize {
    let mut line_start = 0usize;
    for _ in 0..position.line {
        match text[line_start..].find('\n') {
            Some(i) => line_start += i + 1,
            None => return text.len(),
        }
    }
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |i| line_start + i);
    let mut units = 0u32;
    for (i, c) in text[line_start..line_end].char_indices() {
        if units >= position.character {
            return line_start + i;
        }
        units += lsp_u32(c.len_utf16());
    }
    line_end
}

/// The range of a rule's `match` block.
fn span_range(text: &str, check: &RuleCheck) -> Range {
    Range::new(
        position_at(text, check.span.start),
        position_at(text, check.span.end),
    )
}

impl Backend {
    fn completions_for(&self, prefix: &str) -> Vec<CompletionItem> {
        let trimmed = prefix.trim_start();

        // `match … { <cursor>` — we're inside an action block.
        // Check this *before* the bare `match ` prefix because that
        // line also starts with `match `.
        if trimmed.starts_with("match ") && trimmed.contains('{') {
            return action_completions();
        }

        // After `match <ns> "<target>" ` → the clauses that come before
        // the braces: where the rule applies, and `where`.
        if let Some(after_match) = trimmed.strip_prefix("match ")
            && target_closed(after_match)
        {
            return CLAUSES
                .iter()
                .map(|(name, snippet, detail)| CompletionItem {
                    label: (*name).into(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    insert_text: Some((*snippet).into()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    detail: Some((*detail).into()),
                    ..Default::default()
                })
                .collect();
        }

        // After `match ` but no `{` yet → namespace / module.
        if let Some(after_match) = trimmed.strip_prefix("match ") {
            return self.namespace_or_module_completions(after_match.trim_start());
        }

        // Inside a `refactor … { <cursor>` block — suggest `match`.
        if trimmed.contains('{') {
            return vec![CompletionItem {
                label: "match".into(),
                kind: Some(CompletionItemKind::KEYWORD),
                insert_text: Some("match $1::$2 \"$3\" { $0 }".into()),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                detail: Some("Match block".into()),
                ..Default::default()
            }];
        }

        // Top-level: only `refactor` is meaningful.
        if "refactor".starts_with(trimmed) && !trimmed.is_empty() {
            return vec![CompletionItem {
                label: "refactor".into(),
                kind: Some(CompletionItemKind::KEYWORD),
                insert_text: Some("refactor \"$1\" {\n    $0\n}".into()),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                detail: Some("Top-level codemod block".into()),
                ..Default::default()
            }];
        }
        Vec::new()
    }

    /// Suggest a namespace (`<lang>`) when nothing follows yet, or
    /// the available modules for that backend once `<lang>::` is
    /// typed.
    fn namespace_or_module_completions(&self, fragment: &str) -> Vec<CompletionItem> {
        if let Some((lang, _module_prefix)) = fragment.split_once("::") {
            let Some(backend) = self.backends.get(lang) else {
                return Vec::new();
            };
            return backend
                .capabilities()
                .iter()
                .map(|cap| CompletionItem {
                    label: cap.module.into(),
                    kind: Some(CompletionItemKind::MODULE),
                    detail: Some(cap.description.into()),
                    ..Default::default()
                })
                .collect();
        }

        // No `::` typed yet — suggest a language. Each entry inserts
        // `<lang>::` so the next completion can offer modules.
        self.backends
            .languages()
            .iter()
            .map(|lang| CompletionItem {
                label: (*lang).into(),
                kind: Some(CompletionItemKind::MODULE),
                insert_text: Some(format!("{lang}::")),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                detail: Some("colab backend namespace".into()),
                ..Default::default()
            })
            .collect()
    }
}

/// The clauses between a rule's target and its braces.
const CLAUSES: &[(&str, &str, &str)] = &[
    ("in \"<glob>\"", "in \"$0\"", "only files matching the glob"),
    (
        "in fences",
        "in fences",
        "only inside Markdown code fences of this language",
    ),
    (
        "and fences",
        "and fences",
        "this language's files and its Markdown code fences",
    ),
    (
        "where",
        "where $0",
        "constraints on a pattern's metavariables",
    ),
];

/// `true` when `after_match` (the text after `match `) has a namespace,
/// a whole target string, and a space after it: the cursor is where the
/// clauses go.
fn target_closed(after_match: &str) -> bool {
    let Some((_namespace, rest)) = after_match.split_once(char::is_whitespace) else {
        return false;
    };
    let rest = rest.trim_start();
    let close = if let Some(raw) = rest.strip_prefix("r##\"") {
        raw.find("\"##").map(|i| 4 + i + 3)
    } else if let Some(raw) = rest.strip_prefix("r#\"") {
        raw.find("\"#").map(|i| 3 + i + 2)
    } else {
        rest.chars()
            .next()
            .filter(|q| *q == '"' || *q == '\'')
            .and_then(|q| rest[1..].find(q).map(|i| i + 2))
    };
    close.is_some_and(|end| rest[end..].starts_with(char::is_whitespace))
}

/// Import clauses, offered alongside the actions: they follow one inside
/// the braces.
const IMPORT_CLAUSES: &[(&str, &str)] = &[
    ("requires import", "requires import \"$0\""),
    ("drops import", "drops import \"$0\" when unused"),
    ("message", "message \"$0\""),
    ("severity", "severity ${1|error,warning,info|}"),
    ("let", "let \\$${1:NAME} = \\$${2:FROM} | ${3:snake}"),
];

fn action_completions() -> Vec<CompletionItem> {
    let item = |detail: &str, (name, snippet): &(&str, &str)| CompletionItem {
        label: (*name).into(),
        kind: Some(CompletionItemKind::KEYWORD),
        insert_text: Some((*snippet).into()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        detail: Some(detail.into()),
        ..Default::default()
    };
    STATIC_ACTIONS
        .iter()
        .map(|a| item("DSL action", a))
        .chain(
            IMPORT_CLAUSES
                .iter()
                .map(|c| item("clause (`let` before the action, the rest after it)", c)),
        )
        .collect()
}

/// Return the slice of `text` from the start of the line up to
/// `position.character`. Used to drive context-aware completion.
fn line_prefix(text: &str, position: Position) -> &str {
    let line_idx = position.line as usize;
    let mut start = 0usize;
    for _ in 0..line_idx {
        match text[start..].find('\n') {
            Some(p) => start += p + 1,
            None => return "",
        }
    }
    let rest = &text[start..];
    let line_end = rest.find('\n').unwrap_or(rest.len());
    let line = &rest[..line_end];
    let char_offset = (position.character as usize).min(line.len());
    &line[..char_offset]
}

/// Start the LSP server, serving requests over stdio until the client
/// disconnects. The supplied registry is the same one
/// `colab refactor` would use, so the LSP's diagnostics and
/// completion stay in lockstep with the CLI.
pub(crate) async fn run(backends: BackendRegistry) {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| Backend::new(client, backends));
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_prefix_handles_position_inside_first_line() {
        assert_eq!(line_prefix("hello world\n", Position::new(0, 5)), "hello");
    }

    #[test]
    fn line_prefix_handles_second_line() {
        assert_eq!(
            line_prefix("first\nsecond line\n", Position::new(1, 6)),
            "second"
        );
    }

    #[test]
    fn line_prefix_clamps_to_line_length() {
        assert_eq!(line_prefix("ab\n", Position::new(0, 99)), "ab");
    }

    #[test]
    fn is_codemod_uri_recognises_extension() {
        assert!(is_codemod_uri(&"file:///x/y.colab".parse::<Uri>().unwrap()));
        assert!(is_codemod_uri(
            &"file:///x/y.codemod".parse::<Uri>().unwrap()
        ));
        assert!(!is_codemod_uri(&"file:///x/y.go".parse::<Uri>().unwrap()));
    }

    fn registry() -> BackendRegistry {
        colab_backends::registry()
    }

    #[test]
    fn include_at_finds_the_path_under_the_cursor() {
        let text = "refactor \"r\" {\n    include \"packs/a.colab\"\n    match go::include \"x\" { delete }\n}\n";
        let at = text.find("packs").unwrap();
        assert_eq!(include_at(text, at).as_deref(), Some("packs/a.colab"));
        assert_eq!(
            include_at(text, text.find("include").unwrap()).as_deref(),
            Some("packs/a.colab")
        );
        // `c::include` is a module, not the directive.
        assert_eq!(
            include_at(text, text.find("go::include").unwrap() + 5),
            None
        );
        assert_eq!(include_at(text, text.find("match").unwrap()), None);
        assert_eq!(
            include_at("include r#\"b.colab\"#", 12).as_deref(),
            Some("b.colab")
        );
    }

    #[tokio::test]
    async fn an_include_goes_to_the_included_script() {
        let dir = std::env::temp_dir().join(format!("colab-lsp-def-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("packs")).unwrap();
        std::fs::write(dir.join("packs/a.colab"), "refactor \"a\" {}\n").unwrap();
        let main = dir.join("main.colab");
        let text =
            "refactor \"m\" {\n    include \"packs/a.colab\"\n    include \"missing.colab\"\n}\n";
        std::fs::write(&main, text).unwrap();
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let uri = Uri::from_file_path(&main).unwrap();
        backend
            .documents
            .lock()
            .await
            .insert(uri.clone(), text.into());
        let at = |line, character| GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position { line, character },
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let found = backend.goto_definition(at(1, 16)).await.unwrap();
        let Some(GotoDefinitionResponse::Scalar(location)) = found else {
            panic!("no definition: {found:?}");
        };
        assert!(
            location.uri.path().as_str().ends_with("/packs/a.colab"),
            "{location:?}"
        );
        // A file that does not exist goes nowhere.
        assert!(backend.goto_definition(at(2, 16)).await.unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn diagnose_returns_empty_for_valid_script() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let diags =
            backend.diagnose("refactor \"x\" { match go::import \"old\" { replace \"new\" } }");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[tokio::test]
    async fn diagnose_warns_about_a_rewrite_that_matches_its_own_output() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let diags = backend.diagnose(
            "refactor \"x\" {\n    match go::pattern 'f($A)' { replace 'f(g($A))' }\n}\n",
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(diags[0].message.contains("matches the pattern again"));
        assert_eq!(diags[0].range.start.line, 1, "on the rule's line");
    }

    #[tokio::test]
    async fn diagnose_emits_parse_error() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let diags = backend.diagnose("this is not a script");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diags[0].code, Some(NumberOrString::Number(2)));
    }

    #[tokio::test]
    async fn diagnose_emits_unsupported_namespace_error() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let diags =
            backend.diagnose("refactor \"x\" { match klingon::module \"a\" { replace \"b\" } }");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, Some(NumberOrString::Number(3)));
    }

    #[tokio::test]
    async fn completions_after_match_offer_languages() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let items = backend.completions_for("    match ");
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"go"));
        assert!(labels.contains(&"rust"));
    }

    #[tokio::test]
    async fn completions_after_lang_offer_modules() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let items = backend.completions_for("    match go::");
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"import"));
        assert!(labels.contains(&"symbol"));
    }

    #[tokio::test]
    async fn completions_after_the_target_offer_the_clauses() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let labels = |prefix: &str| -> Vec<String> {
            backend
                .completions_for(prefix)
                .into_iter()
                .map(|i| i.label)
                .collect()
        };
        let after = labels("    match go::pattern 'errors.Wrap($E, $M)' ");
        assert!(after.contains(&"in fences".to_string()), "{after:?}");
        assert!(after.contains(&"and fences".to_string()));
        assert!(after.contains(&"where".to_string()));
        assert!(
            labels("    match go::import \"a/b")
                .iter()
                .all(|l| l != "in fences"),
            "still in the string"
        );
        assert!(labels("    match go::import r#\"a\"# ").contains(&"in fences".to_string()));
    }

    #[tokio::test]
    async fn completions_inside_block_offer_actions() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let items = backend.completions_for("        match go::import \"old\" { ");
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"replace"));
        assert!(labels.contains(&"delete"));
        assert!(labels.contains(&"ensure"));
        assert!(labels.contains(&"replace_call"));
        assert!(labels.contains(&"requires import"));
    }

    #[tokio::test]
    async fn diagnose_places_each_bad_rule_on_its_own_block() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let text = "refactor \"x\" {\n  match go::import \"a\" { replace \"b\" }\n  match go::improt \"a\" { delete }\n  match go::pattern 'func (' { delete }\n}\n";
        let diags = service.inner().diagnose(text);
        assert_eq!(diags.len(), 2, "{diags:?}");
        assert_eq!(diags[0].range.start, Position::new(2, 2));
        assert!(
            diags[0].message.contains("did you mean `import`"),
            "{}",
            diags[0].message
        );
        assert_eq!(diags[1].range.start, Position::new(3, 2));
        assert!(
            diags[1].message.contains("go::pattern"),
            "{}",
            diags[1].message
        );
    }

    #[tokio::test]
    async fn hover_explains_modules_and_actions_in_context() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let text = "refactor \"x\" {\n  match go::pattern 'f($A)' { delete }\n}\n";
        let on_module = text.find("pattern").unwrap();
        let hover = backend.hover_at(text, on_module).unwrap();
        assert!(hover.starts_with("**`go::pattern`**"), "{hover}");
        assert!(hover.contains("- `replace`"), "{hover}");
        let on_action = text.find("delete").unwrap() + 2;
        let hover = backend.hover_at(text, on_action).unwrap();
        assert!(
            hover.starts_with("**`delete`** on `go::pattern`"),
            "{hover}"
        );
        assert!(
            backend
                .hover_at(text, text.find("f($A)").unwrap() + 3)
                .is_none()
        );
    }

    #[tokio::test]
    async fn hover_says_when_an_action_does_not_apply() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let text = "refactor \"x\" { match go::symbol \"A\" { delete } }";
        let hover = service
            .inner()
            .hover_at(text, text.find("delete").unwrap())
            .unwrap();
        assert!(hover.contains("has no `delete` action"), "{hover}");
    }

    #[test]
    fn positions_round_trip_through_utf16() {
        let text = "ab\nµ€x\n";
        let x = text.find('x').unwrap();
        let pos = position_at(text, x);
        assert_eq!(pos, Position::new(1, 2), "µ and € are one UTF-16 unit each");
        assert_eq!(offset_at(text, pos), x);
        assert_eq!(offset_at(text, Position::new(9, 0)), text.len());
    }

    #[test]
    fn word_at_spans_namespaces() {
        let text = "match go::import \"a\"";
        assert_eq!(word_at(text, 8), Some("go::import"));
        assert_eq!(word_at(text, 0), Some("match"));
        assert_eq!(word_at(text, 5), Some("match"));
    }

    /// Apply an LSP `changes` map to the files on disk, as an editor would.
    fn apply_on_disk(edit: &serde_json::Value) {
        for (uri, edits) in edit["changes"].as_object().into_iter().flatten() {
            let path = uri
                .parse::<Uri>()
                .unwrap()
                .to_file_path()
                .unwrap()
                .into_owned();
            let mut text = std::fs::read_to_string(&path).unwrap();
            let mut edits: Vec<TextEdit> = serde_json::from_value(edits.clone()).unwrap();
            edits.sort_by_key(|e| std::cmp::Reverse((e.range.start.line, e.range.start.character)));
            for e in edits {
                let (from, to) = (
                    offset_at(&text, e.range.start),
                    offset_at(&text, e.range.end),
                );
                text.replace_range(from..to, &e.new_text);
            }
            std::fs::write(&path, text).unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn apply_then_undo_keeps_a_later_edit_elsewhere_in_the_file() {
        use futures::{SinkExt, StreamExt};
        let root = std::env::temp_dir().join(format!("colab-lsp-undo-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("main.go");
        std::fs::write(&file, "package main\n\nimport \"a\"\n\nfunc main() {}\n").unwrap();

        let (service, socket) = LspService::new(|client| Backend::new(client, registry()));
        // The editor: apply every `workspace/applyEdit` to disk, say yes.
        let (mut requests, mut responses) = socket.split();
        tokio::spawn(async move {
            while let Some(request) = requests.next().await {
                let applied = request.method() == "workspace/applyEdit";
                if applied {
                    apply_on_disk(&request.params().unwrap()["edit"]);
                }
                if let Some(id) = request.id().cloned() {
                    let body = if applied {
                        serde_json::json!({"applied": true})
                    } else {
                        serde_json::Value::Null
                    };
                    let _ = responses
                        .send(tower_lsp_server::jsonrpc::Response::from_ok(id, body))
                        .await;
                }
            }
        });
        let mut service = service;
        let initialize = tower_lsp_server::jsonrpc::Request::build("initialize")
            .params(serde_json::json!({
                "capabilities": {"workspace": {
                    "applyEdit": true,
                    "workspaceEdit": {"documentChanges": true, "changeAnnotationSupport": {}},
                }},
                "rootUri": Uri::from_file_path(&root).unwrap(),
            }))
            .id(1)
            .finish();
        tower::ServiceExt::ready(&mut service).await.unwrap();
        tower::Service::call(&mut service, initialize)
            .await
            .unwrap();
        let initialized = tower_lsp_server::jsonrpc::Request::build("initialized")
            .params(serde_json::json!({}))
            .finish();
        tower::Service::call(&mut service, initialized)
            .await
            .unwrap();
        let backend = service.inner();

        let uri = Uri::from_file_path(root.join("fix.colab")).unwrap();
        let text = "refactor \"x\" { match go::import \"a\" { replace \"b\" } }\n";
        backend
            .documents
            .lock()
            .await
            .insert(uri.clone(), text.into());
        let titles = |actions: Option<CodeActionResponse>| -> Vec<String> {
            actions
                .unwrap_or_default()
                .into_iter()
                .map(|a| match a {
                    CodeActionOrCommand::CodeAction(a) => a.title,
                    CodeActionOrCommand::Command(c) => c.title,
                })
                .collect()
        };
        let ask = || CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range::new(Position::new(0, 20), Position::new(0, 20)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let offered = titles(backend.code_action(ask()).await.unwrap());
        assert!(
            offered.iter().any(|t| t.starts_with("colab: preview `")),
            "{offered:?}"
        );
        assert!(
            offered.iter().any(|t| t.starts_with("colab: apply `")),
            "{offered:?}"
        );
        assert!(!offered.iter().any(|t| t.contains("undo")), "{offered:?}");

        let run = |command: &str, arguments: Vec<serde_json::Value>| ExecuteCommandParams {
            command: command.into(),
            arguments,
            work_done_progress_params: Default::default(),
        };
        let rule = serde_json::json!({"uri": uri.as_str(), "rule": 0, "label": "`a -> b`"});
        backend
            .execute_command(run(APPLY_COMMAND, vec![rule]))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "package main\n\nimport \"b\"\n\nfunc main() {}\n"
        );
        let offered = titles(backend.code_action(ask()).await.unwrap());
        assert!(
            offered.contains(&"colab: undo applying `a -> b`".to_string()),
            "{offered:?}"
        );

        // A later edit, away from the run's: the undo keeps it.
        std::fs::write(
            &file,
            "package main\n\nimport \"b\"\n\nfunc main() { run() }\n",
        )
        .unwrap();
        backend
            .execute_command(run(UNDO_COMMAND, vec![]))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "package main\n\nimport \"a\"\n\nfunc main() { run() }\n"
        );
        assert!(backend.undo.lock().await.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_long_workspace_edit_reports_progress() {
        use futures::{SinkExt, StreamExt};
        let root = std::env::temp_dir().join(format!("colab-lsp-progress-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..(PROGRESS_EVERY + 5) {
            std::fs::write(root.join(format!("f{i}.go")), "package p\n\nimport \"a\"\n").unwrap();
        }
        let (service, socket) = LspService::new(|client| Backend::new(client, registry()));
        // The client: answer every request, keep what the server sends.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let (mut requests, mut responses) = socket.split();
        tokio::spawn(async move {
            while let Some(request) = requests.next().await {
                let kind = request
                    .params()
                    .and_then(|p| p.pointer("/value/kind"))
                    .and_then(|k| k.as_str())
                    .unwrap_or("")
                    .to_string();
                log.lock()
                    .unwrap()
                    .push(format!("{} {kind}", request.method()));
                if let Some(id) = request.id().cloned() {
                    let _ = responses
                        .send(tower_lsp_server::jsonrpc::Response::from_ok(
                            id,
                            serde_json::Value::Null,
                        ))
                        .await;
                }
            }
        });
        // A real handshake: the client sends nothing to the server's
        // client until the session is initialized.
        let mut service = service;
        let initialize = tower_lsp_server::jsonrpc::Request::build("initialize")
            .params(serde_json::json!({
                "capabilities": {"window": {"workDoneProgress": true}},
                "rootUri": Uri::from_file_path(&root).unwrap(),
            }))
            .id(1)
            .finish();
        tower::ServiceExt::ready(&mut service).await.unwrap();
        tower::Service::call(&mut service, initialize)
            .await
            .unwrap();
        let initialized = tower_lsp_server::jsonrpc::Request::build("initialized")
            .params(serde_json::json!({}))
            .finish();
        tower::Service::call(&mut service, initialized)
            .await
            .unwrap();
        let backend = service.inner();
        assert!(
            backend.progress.load(Ordering::Relaxed),
            "read from initialize"
        );
        let uri = Uri::from_file_path(root.join("fix.colab")).unwrap();
        let text = "refactor \"x\" { match go::import \"a\" { replace \"b\" } }\n";
        let edit = backend.workspace_edit(&uri, text, None).await.unwrap();
        assert_eq!(edit.changes.unwrap().len(), PROGRESS_EVERY + 5);
        // The client task may still be reading the last notification.
        let progress = || -> Vec<String> {
            seen.lock()
                .unwrap()
                .iter()
                .filter(|m| m.contains("rogress"))
                .cloned()
                .collect()
        };
        for _ in 0..100 {
            if progress().last().is_some_and(|m| m.ends_with("end")) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let seen = progress();
        assert_eq!(seen[0], "window/workDoneProgress/create ", "{seen:?}");
        assert_eq!(seen[1], "$/progress begin", "{seen:?}");
        assert!(seen.contains(&"$/progress report".to_string()), "{seen:?}");
        assert_eq!(seen.last().unwrap(), "$/progress end", "{seen:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workspace_edit_rewrites_files_under_the_roots_without_writing() {
        let root = std::env::temp_dir().join(format!("colab-lsp-edit-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("main.go");
        std::fs::write(&file, "package main\n\nimport \"a\"\nimport \"c\"\n").unwrap();

        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        *backend.roots.lock().await = vec![root.clone()];
        let uri = Uri::from_file_path(root.join("fix.colab")).unwrap();
        let text = "refactor \"x\" {\n  match go::import \"a\" { replace \"b\" }\n  match go::import \"c\" { replace \"d\" }\n}\n";

        let all = backend.workspace_edit(&uri, text, None).await.unwrap();
        let only_second = backend.workspace_edit(&uri, text, Some(1)).await.unwrap();
        let file_uri = Uri::from_file_path(&file).unwrap();
        // Hunks, not the whole file: only the import lines change.
        assert_eq!(
            all.changes.as_ref().unwrap()[&file_uri],
            [TextEdit::new(
                Range::new(Position::new(2, 0), Position::new(4, 0)),
                "import \"b\"\nimport \"d\"\n".into()
            )]
        );
        assert_eq!(
            only_second.changes.as_ref().unwrap()[&file_uri],
            [TextEdit::new(
                Range::new(Position::new(3, 0), Position::new(4, 0)),
                "import \"d\"\n".into()
            )]
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "package main\n\nimport \"a\"\nimport \"c\"\n",
            "the server never writes; the editor applies the edit"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
