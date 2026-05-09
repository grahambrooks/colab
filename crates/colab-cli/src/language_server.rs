//! LSP server for `.codemod` scripts.
//!
//! Today serves two features:
//!
//! - **Diagnostics.** Every open / change of a `.codemod` file
//!   triggers `colab_dsl::compile` against the binary's default
//!   [`BackendRegistry`]; parse errors and unsupported-namespace
//!   errors surface as LSP diagnostics. Position info is lost when
//!   `Error::Parse` collapses the LALRPOP error to a string, so for
//!   now diagnostics point at line 0; the message text retains the
//!   parser's `at byte N` pointer.
//! - **Completion.** Suggests namespaces (`go::`, `rust::`, …),
//!   modules (per backend), and actions (`replace`/`delete`/`ensure`/
//!   `replace_call`) based on cursor context. Items are sourced from
//!   `BackendRegistry::capabilities` so completion stays in lockstep
//!   with `colab list-rules`.
//!
//! Hover, go-to-definition, and the "preview" code action are
//! deferred — see the development plan §3.6.

use std::collections::HashMap;

use tokio::sync::Mutex as AsyncMutex;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use colab_core::BackendRegistry;
use colab_dsl::compile;

const STATIC_ACTIONS: &[(&str, &str)] = &[
    ("replace", "replace \"$0\""),
    ("delete", "delete"),
    ("ensure", "ensure"),
    ("replace_call", "replace_call \"$0\""),
];

/// Tower-LSP backend. Holds the open-document store and a backend
/// registry the diagnostics + completion handlers consult.
struct Backend {
    client: Client,
    documents: AsyncMutex<HashMap<Url, String>>,
    backends: BackendRegistry,
}

impl Backend {
    fn new(client: Client, backends: BackendRegistry) -> Self {
        Self {
            client,
            documents: AsyncMutex::new(HashMap::new()),
            backends,
        }
    }

    /// Run `colab_dsl::compile` and convert any [`Error`] into an LSP
    /// diagnostic at line 0. Empty diagnostics list means the script
    /// compiled cleanly.
    fn diagnose(&self, text: &str) -> Vec<Diagnostic> {
        match compile(text, &self.backends) {
            Ok(_) => Vec::new(),
            Err(err) => vec![Diagnostic {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::Number(err.exit_code())),
                source: Some("colab".into()),
                message: format!("{}", err),
                related_information: None,
                tags: None,
                code_description: None,
                data: None,
            }],
        }
    }

    async fn refresh_diagnostics(&self, uri: Url, version: Option<i32>, text: String) {
        // Only diagnose `.codemod` files; ignore everything else so
        // editors that route arbitrary buffers here don't pollute
        // the diagnostics panel.
        if !is_codemod_uri(&uri) {
            self.documents.lock().await.insert(uri, text);
            return;
        }
        let diagnostics = self.diagnose(&text);
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
    async fn initialize(&self, _: InitializeParams) -> LspResult<InitializeResult> {
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
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(
                MessageType::INFO,
                format!(
                    "colab LSP ready; backends: {:?}",
                    self.backends.languages()
                ),
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

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> LspResult<Option<CompletionResponse>> {
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
        let mut r = BackendRegistry::new();
        r.register(Box::new(colab_lang_go::GoBackend));
        r.register(Box::new(colab_lang_rust::RustBackend));
        r
    }

    #[tokio::test]
    async fn diagnose_returns_empty_for_valid_script() {
        let (service, _) = LspService::new(|client| Backend::new(client, registry()));
        let backend = service.inner();
        let diags = backend.diagnose(
            "refactor \"x\" { match go::import \"old\" { replace \"new\" } }",
        );
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
        let diags = backend.diagnose(
            "refactor \"x\" { match klingon::module \"a\" { replace \"b\" } }",
        );
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
}
