//! LSP server for `.codemod` scripts.
//!
//! Today serves two features:
//!
//! - **Diagnostics.** Every open / change of a `.codemod` file
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex as AsyncMutex;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

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
            let line = detail.line.saturating_sub(1) as u32;
            let character = detail.column.saturating_sub(1) as u32;
            Range::new(
                Position::new(line, character),
                Position::new(line, character + 1),
            )
        }
        _ => Range::new(Position::new(0, 0), Position::new(0, 0)),
    }
}

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
    documents: AsyncMutex<HashMap<Url, String>>,
    backends: BackendRegistry,
    /// Workspace roots from `initialize`; what the code actions rewrite.
    roots: AsyncMutex<Vec<PathBuf>>,
    /// The client resolves code-action edits lazily (`resolveSupport`
    /// lists `edit`). Without it the edit is computed up front.
    lazy_edits: AtomicBool,
}

fn diagnostic(range: Range, err: &Error) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::Number(err.exit_code())),
        source: Some("colab".into()),
        message: format!("{}", err),
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

    async fn refresh_diagnostics(&self, uri: Url, version: Option<i32>, text: String) {
        // Only diagnose `.codemod` files; ignore everything else so
        // editors that route arbitrary buffers here don't pollute
        // the diagnostics panel.
        if !is_codemod_uri(&uri) {
            self.documents.lock().await.insert(uri, text);
            return;
        }
        let path = uri.to_file_path().ok();
        let diagnostics = self.diagnose_at(path.as_deref(), &text);
        self.documents.lock().await.insert(uri.clone(), text);
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }
}

fn is_codemod_uri(uri: &Url) -> bool {
    uri.path().ends_with(".codemod")
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> LspResult<InitializeResult> {
        let mut roots: Vec<PathBuf> = params
            .workspace_folders
            .unwrap_or_default()
            .iter()
            .filter_map(|f| f.uri.to_file_path().ok())
            .collect();
        #[allow(deprecated)] // `root_uri` is still what many clients send
        if roots.is_empty()
            && let Some(root) = params.root_uri.and_then(|u| u.to_file_path().ok())
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
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![CodeActionKind::REFACTOR_REWRITE]),
                        resolve_provider: Some(true),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
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
        let mut actions = Vec::new();
        if let Some((index, check)) = checks
            .iter()
            .enumerate()
            .find(|(_, c)| c.span.start <= offset && offset <= c.span.end)
        {
            let description = check.outcome.as_ref().map(String::as_str).unwrap_or("");
            actions.push(self.workspace_action(
                format!("colab: apply `{description}` to the workspace"),
                &uri,
                Some(index),
            ));
        }
        actions.push(self.workspace_action(
            "colab: apply the whole script to the workspace".into(),
            &uri,
            None,
        ));
        if !self.lazy_edits.load(Ordering::Relaxed) {
            for action in &mut actions {
                let rule = action_rule(action);
                action.edit = Some(
                    self.workspace_edit(&uri, &text, rule)
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
            .and_then(|u| Url::parse(u).ok())
        else {
            return Ok(action);
        };
        let Some(text) = self.documents.lock().await.get(&uri).cloned() else {
            return Ok(action);
        };
        let rule = action_rule(&action);
        action.edit = Some(
            self.workspace_edit(&uri, &text, rule)
                .await
                .map_err(lsp_error)?,
        );
        Ok(action)
    }
}

/// The rule index a workspace code action was created for (`None` = all).
fn action_rule(action: &CodeAction) -> Option<usize> {
    action
        .data
        .as_ref()
        .and_then(|d| d.get("rule"))
        .and_then(|r| r.as_u64())
        .map(|r| r as usize)
}

fn lsp_error(err: Error) -> tower_lsp::jsonrpc::Error {
    let mut out = tower_lsp::jsonrpc::Error::internal_error();
    out.message = err.to_string().into();
    out
}

impl Backend {
    fn workspace_action(&self, title: String, uri: &Url, rule: Option<usize>) -> CodeAction {
        CodeAction {
            title,
            kind: Some(CodeActionKind::REFACTOR_REWRITE),
            data: Some(serde_json::json!({ "uri": uri.as_str(), "rule": rule })),
            ..Default::default()
        }
    }

    /// Run the script — or just rule `rule` — over every workspace root and
    /// return the rewrites as one `WorkspaceEdit`, a whole-document edit
    /// per changed file. Nothing is written: the editor applies it, after
    /// whatever preview it offers.
    async fn workspace_edit(
        &self,
        uri: &Url,
        text: &str,
        rule: Option<usize>,
    ) -> colab_core::Result<WorkspaceEdit> {
        let base = uri
            .to_file_path()
            .unwrap_or_else(|_| PathBuf::from("script.codemod"));
        let mut refactoring = compile_at_source(text, &base, &self.backends)?;
        if let Some(index) = rule {
            if index >= refactoring.rules.len() {
                return Err(Error::Config(format!("the script has no rule {index}")));
            }
            let only = refactoring.rules.swap_remove(index);
            refactoring.rules = vec![only];
        }
        let mut roots = self.roots.lock().await.clone();
        if roots.is_empty()
            && let Some(parent) = base.parent()
        {
            roots.push(parent.to_path_buf());
        }
        // The walk is blocking, CPU-bound work on a runtime worker.
        let changes = tokio::task::block_in_place(|| {
            let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
            for root in &roots {
                walker::walk_with(&refactoring, root, &WalkOptions::default(), &mut |change| {
                    if change.changed()
                        && let Ok(url) = Url::from_file_path(&change.path)
                    {
                        let end = position_at(&change.before, change.before.len());
                        changes.insert(
                            url,
                            vec![TextEdit::new(
                                Range::new(Position::new(0, 0), end),
                                change.after.clone(),
                            )],
                        );
                    }
                    Ok(())
                })?;
            }
            Ok::<_, Error>(changes)
        })?;
        Ok(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
    }

    /// Markdown for the token at byte `offset`, if colab knows it.
    fn hover_at(&self, text: &str, offset: usize) -> Option<String> {
        let word = word_at(text, offset)?;
        if let Some((lang, module)) = word.split_once("::") {
            let backend = self.backends.get(lang)?;
            let cap = backend.capabilities().iter().find(|c| c.module == module)?;
            let mut out = format!("**`{lang}::{module}`** — {}\n\nActions:\n", cap.description);
            for action in cap.actions {
                out.push_str(&format!("- `{}` — {}\n", action.name, action.description));
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
        .map(|(i, _)| i)
        .unwrap_or(offset);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map(|(i, _)| offset + i)
        .unwrap_or(text.len());
    let word = text[start..end].trim_matches(':');
    (!word.is_empty()).then_some(word)
}

/// LSP position (0-based line, UTF-16 column) of byte `offset`.
fn position_at(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = text[line_start..offset].encode_utf16().count() as u32;
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
        .map(|i| line_start + i)
        .unwrap_or(text.len());
    let mut units = 0u32;
    for (i, c) in text[line_start..line_end].char_indices() {
        if units >= position.character {
            return line_start + i;
        }
        units += c.len_utf16() as u32;
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
                insert_text: Some(format!("{}::", lang)),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                detail: Some("colab backend namespace".into()),
                ..Default::default()
            })
            .collect()
    }
}

fn action_completions() -> Vec<CompletionItem> {
    STATIC_ACTIONS
        .iter()
        .map(|(name, snippet)| CompletionItem {
            label: (*name).into(),
            kind: Some(CompletionItemKind::KEYWORD),
            insert_text: Some((*snippet).into()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            detail: Some("DSL action".into()),
            ..Default::default()
        })
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
        assert!(is_codemod_uri(&Url::parse("file:///x/y.codemod").unwrap()));
        assert!(!is_codemod_uri(&Url::parse("file:///x/y.go").unwrap()));
    }

    fn registry() -> BackendRegistry {
        colab_backends::registry()
    }

    #[tokio::test]
    async fn diagnose_returns_empty_for_valid_script() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let diags =
            backend.diagnose("refactor \"x\" { match go::import \"old\" { replace \"new\" } }");
        assert!(diags.is_empty(), "got: {:?}", diags);
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
    async fn completions_inside_block_offer_actions() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let items = backend.completions_for("        match go::import \"old\" { ");
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"replace"));
        assert!(labels.contains(&"delete"));
        assert!(labels.contains(&"ensure"));
        assert!(labels.contains(&"replace_call"));
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

    #[tokio::test(flavor = "multi_thread")]
    async fn workspace_edit_rewrites_files_under_the_roots_without_writing() {
        let root = std::env::temp_dir().join(format!("colab-lsp-edit-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("main.go");
        std::fs::write(&file, "package main\n\nimport \"a\"\nimport \"c\"\n").unwrap();

        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        *backend.roots.lock().await = vec![root.clone()];
        let uri = Url::from_file_path(root.join("fix.codemod")).unwrap();
        let text = "refactor \"x\" {\n  match go::import \"a\" { replace \"b\" }\n  match go::import \"c\" { replace \"d\" }\n}\n";

        let all = backend.workspace_edit(&uri, text, None).await.unwrap();
        let only_second = backend.workspace_edit(&uri, text, Some(1)).await.unwrap();
        let file_uri = Url::from_file_path(&file).unwrap();
        let after_all = &all.changes.as_ref().unwrap()[&file_uri][0];
        assert_eq!(
            after_all.new_text,
            "package main\n\nimport \"b\"\nimport \"d\"\n"
        );
        assert_eq!(after_all.range.end, Position::new(4, 0));
        let after_one = &only_second.changes.as_ref().unwrap()[&file_uri][0];
        assert_eq!(
            after_one.new_text,
            "package main\n\nimport \"a\"\nimport \"d\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "package main\n\nimport \"a\"\nimport \"c\"\n",
            "the server never writes; the editor applies the edit"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
