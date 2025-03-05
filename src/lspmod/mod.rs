use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use tower_lsp::lsp_types::*;
use tower_lsp::{LspService, Server};
use serde::{Deserialize, Serialize};
use std::error::Error;

#[derive(Debug, Serialize, Deserialize)]
pub struct ImportChangeRequest {
    pub file_path: String,
    pub old_import: String,
    pub new_import: String,
}

pub struct GoplsManager {
    process: Option<Child>,
}

impl GoplsManager {
    pub fn new() -> Self {
        Self { process: None }
    }

    fn ensure_gopls_installed() -> Result<(), Box<dyn Error>> {
        // Check if gopls is installed
        let status = Command::new("gopls")
            .arg("version")
            .status()?;

        if !status.success() {
            // Install gopls if not present
            Command::new("go")
                .args(&["install", "golang.org/x/tools/gopls@latest"])
                .status()?;
        }
        Ok(())
    }

    fn start_server(&mut self) -> Result<(), Box<dyn Error>> {
        Self::ensure_gopls_installed()?;

        let process = Command::new("gopls")
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        self.process = Some(process);
        Ok(())
    }

    pub async fn change_import(&mut self, request: ImportChangeRequest) -> Result<String, Box<dyn Error>> {
        if self.process.is_none() {
            self.start_server()?;
        }

        let process = self.process.as_mut().unwrap();
        
        // Create LSP service
        let (service, socket) = LspService::build(|client| Backend { client })
            .finish();

        // Start server
        let stdin = process.stdin.take().unwrap();
        let stdout = process.stdout.take().unwrap();
        
        let server = Server::new(stdin, stdout, socket);
        
        // Initialize the connection
        let initialize_params = InitializeParams {
            root_uri: Some(Url::from_file_path(&request.file_path).unwrap()),
            capabilities: ClientCapabilities::default(),
            ..InitializeParams::default()
        };

        // Send initialize request
        service.initialize(initialize_params).await?;

        // Create workspace edit for import change
        let edit = WorkspaceEdit {
            changes: Some({
                let mut changes = HashMap::new();
                changes.insert(
                    Url::from_file_path(&request.file_path).unwrap(),
                    vec![TextEdit {
                        range: Range {
                            start: Position {
                                line: 0,
                                character: 0,
                            },
                            end: Position {
                                line: 0,
                                character: 0,
                            },
                        },
                        new_text: format!("import \"{}\"", request.new_import),
                    }],
                );
                changes
            }),
            document_changes: None,
            change_annotations: None,
        };

        // Apply the edit
        service.apply_edit(edit).await?;

        // Read the modified file
        let modified_content = std::fs::read_to_string(&request.file_path)?;
        
        Ok(modified_content)
    }
}

// LSP Backend implementation
struct Backend {
    client: tower_lsp::Client,
}

#[tower_lsp::async_trait]
impl tower_lsp::LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult, tower_lsp::jsonrpc::Error> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncKind::FULL.into()),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client.log_message(MessageType::INFO, "server initialized!").await;
    }

    async fn shutdown(&self) -> Result<(), tower_lsp::jsonrpc::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_import_change() -> Result<(), Box<dyn Error>> {
        // Create a temporary Go file
        let temp_file = NamedTempFile::new()?;
        let file_path = temp_file.path().to_str().unwrap().to_string();
        
        // Write test Go code
        let test_code = r#"
package main

import "fmt"

func main() {
    fmt.Println("Hello, world!")
}
"#;
        fs::write(&file_path, test_code)?;

        // Create request
        let request = ImportChangeRequest {
            file_path: file_path.clone(),
            old_import: "fmt".to_string(),
            new_import: "fmt".to_string(),
        };

        // Change import
        let mut manager = GoplsManager::new();
        let result = manager.change_import(request).await?;

        // Verify the result
        assert!(result.contains("import \"fmt\""));

        Ok(())
    }
}