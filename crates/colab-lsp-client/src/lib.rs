//! Semantic edits by delegation: colab as a language-server *client*.
//!
//! colab is a syntactic rewriter and stays one. But scope-aware rename —
//! rename *this* `x`, not every `x` — needs name resolution, and every
//! mainstream language already ships a server that does it. This crate
//! drives one (`rust-analyzer`, `gopls`, `clangd`, …) through
//! `textDocument/rename` and hands back plain before/after file pairs, so
//! the result goes through colab's own report, diff, `--check`,
//! `--backup`, and `--verify` exactly like a codemod. colab computes
//! nothing semantic itself; the server does, and colab makes its answer
//! reviewable and revertable.
//!
//! The crate depends on `colab-core` and `colab-rewrite` (for
//! `apply_edits`) only. It is used by the CLI, not by `colab-mcp`, which
//! must stay free of `tree-sitter`.

pub mod edit;
pub mod session;
pub mod transport;

use std::path::{Path, PathBuf};
use std::time::Duration;

use colab_core::walker::FileChange;
use colab_core::{Error, Result};
use serde_json::{Value, json};

pub use edit::Encoding;
use edit::{path_to_uri, position_of};
use session::Session;

/// A language-server command line, e.g. `typescript-language-server --stdio`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl ServerCommand {
    /// Split on whitespace. Quoting is not supported; a server whose path
    /// contains spaces needs a wrapper script.
    pub fn parse(command: &str) -> Result<Self> {
        let mut words = command.split_whitespace().map(str::to_string);
        let program = words
            .next()
            .ok_or_else(|| Error::Config("--lsp needs a command".into()))?;
        Ok(Self {
            program,
            args: words.collect(),
        })
    }

    /// The usual server for `path`'s language, if there is a clear default.
    pub fn default_for(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        let command = match ext {
            "rs" => "rust-analyzer",
            "go" => "gopls",
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "clangd",
            "py" => "pyright-langserver --stdio",
            "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => "typescript-language-server --stdio",
            "swift" => "sourcekit-lsp",
            "rb" => "ruby-lsp",
            "kt" | "kts" => "kotlin-language-server",
            "java" => "jdtls",
            "cs" => "csharp-ls",
            "php" => "intelephense --stdio",
            _ => return None,
        };
        Self::parse(command).ok()
    }

    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The LSP `languageId` for a file, by extension.
fn language_id(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rust",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "py" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "swift" => "swift",
        "rb" => "ruby",
        "kt" | "kts" => "kotlin",
        "java" => "java",
        "cs" => "csharp",
        "php" => "php",
        _ => "plaintext",
    }
}

/// What to rename and how long to wait for the server.
#[derive(Debug, Clone)]
pub struct RenameRequest {
    /// The project root the server is started in.
    pub root: PathBuf,
    /// Absolute path of the file containing the symbol.
    pub file: PathBuf,
    /// Byte offset of the symbol in that file.
    pub offset: usize,
    pub new_name: String,
    /// Overall budget: start-up, indexing, and the rename itself.
    pub timeout: Duration,
    /// How long the server must be silent, with no work in progress,
    /// before it counts as ready.
    pub settle: Duration,
}

/// The server's answer, as files colab can report on and write.
#[derive(Debug)]
pub struct RenameOutcome {
    /// Changed files, sorted by path. Each records rule index 0.
    pub changes: Vec<FileChange>,
    /// The symbol's current name, when the server or the file says.
    pub old_name: Option<String>,
}

/// Ask `server` to rename the symbol at `request.offset`.
pub fn rename(server: &ServerCommand, request: &RenameRequest) -> Result<RenameOutcome> {
    let text =
        std::fs::read_to_string(&request.file).map_err(|e| Error::io_at(&request.file, e))?;
    let mut session = Session::start(
        &server.program,
        &server.args,
        &request.root,
        request.timeout,
    )?;

    let rename_capability = session
        .capabilities
        .get("renameProvider")
        .cloned()
        .unwrap_or(Value::Null);
    if matches!(rename_capability, Value::Null | Value::Bool(false)) {
        let name = session.name().to_string();
        session.shutdown();
        return Err(Error::UnsupportedOperation(format!(
            "`{name}` does not support rename (no renameProvider)"
        )));
    }

    let uri = path_to_uri(&request.file);
    session.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri, "languageId": language_id(&request.file), "version": 1, "text": text}}),
    )?;
    session.wait_until_quiet(request.settle)?;

    let (line, character) = position_of(&text, request.offset, session.encoding);
    let position =
        json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": character}});
    let mut old_name = identifier_at(&text, request.offset).map(str::to_string);

    if rename_capability
        .get("prepareProvider")
        .and_then(Value::as_bool)
        == Some(true)
    {
        let prepared = session
            .request_retrying("textDocument/prepareRename", position.clone())?
            .map_err(|e| session.server_error("textDocument/prepareRename", &e))?;
        if prepared.is_null() {
            let name = session.name().to_string();
            session.shutdown();
            return Err(Error::Config(format!(
                "`{name}` says there is nothing to rename at {}:{}:{}",
                request.file.display(),
                line + 1,
                character + 1
            )));
        }
        if let Some(placeholder) = prepared.get("placeholder").and_then(Value::as_str) {
            old_name = Some(placeholder.to_string());
        }
    }

    let mut params = position;
    params["newName"] = json!(request.new_name);
    let edit = session
        .request_retrying("textDocument/rename", params)?
        .map_err(|e| session.server_error("textDocument/rename", &e))?;
    let encoding = session.encoding;
    let name = session.name().to_string();
    session.shutdown();
    if edit.is_null() {
        return Err(Error::Config(format!(
            "`{name}` returned no edit for the rename"
        )));
    }
    let changes = edit::to_changes(&edit, encoding, |path| {
        std::fs::read_to_string(path).map_err(|e| Error::io_at(path, e))
    })?
    .into_iter()
    .filter(FileChange::changed)
    .collect();
    Ok(RenameOutcome { changes, old_name })
}

/// The identifier around byte `offset`, if any.
fn identifier_at(text: &str, offset: usize) -> Option<&str> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let offset = offset.min(text.len());
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_ident(*c))
        .map_or(text.len(), |(i, _)| offset + i);
    (start < end).then(|| &text[start..end])
}

/// Byte offset of the first whole-identifier occurrence of `name` in
/// `text` — a convenience for naming a symbol instead of a position.
pub fn first_occurrence(text: &str, name: &str) -> Option<usize> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).map(|(i, _)| i).find(|&i| {
        let before_ok = text[..i].chars().next_back().is_none_or(|c| !is_ident(c));
        let after_ok = text[i + name.len()..]
            .chars()
            .next()
            .is_none_or(|c| !is_ident(c));
        before_ok && after_ok
    })
}

/// Byte offset of 1-based `line` and `column` (in characters).
pub fn offset_of_line_column(text: &str, line: usize, column: usize) -> Option<usize> {
    let mut start = 0usize;
    for _ in 1..line {
        start += text[start..].find('\n')? + 1;
    }
    let line_text = &text[start..text[start..].find('\n').map_or(text.len(), |i| start + i)];
    let within = line_text
        .char_indices()
        .nth(column.saturating_sub(1))
        .map_or(line_text.len(), |(i, _)| i);
    Some(start + within)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_occurrence_skips_partial_identifiers() {
        let text = "let my_x = 1;\nlet x = my_x + x;\n";
        assert_eq!(
            first_occurrence(text, "x"),
            Some(text.find("let x").unwrap() + 4)
        );
    }

    #[test]
    fn line_column_is_one_based_in_characters() {
        let text = "ab\nµcd\n";
        assert_eq!(
            offset_of_line_column(text, 2, 2),
            Some(text.find('c').unwrap())
        );
        assert_eq!(offset_of_line_column(text, 9, 1), None);
    }

    #[test]
    fn server_commands_split_and_default_by_extension() {
        let cmd = ServerCommand::parse("pyright-langserver --stdio").unwrap();
        assert_eq!(cmd.program, "pyright-langserver");
        assert_eq!(cmd.args, vec!["--stdio"]);
        assert_eq!(
            ServerCommand::default_for(Path::new("a/b.rs"))
                .unwrap()
                .program,
            "rust-analyzer"
        );
        assert!(ServerCommand::default_for(Path::new("x.toml")).is_none());
    }
}
