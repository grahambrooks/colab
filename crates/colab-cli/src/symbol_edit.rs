//! `colab edit --symbol <name path> --in <file>`: a one-off edit addressed
//! by declaration, not by exact text.
//!
//! ```sh
//! colab edit --symbol Config/load --in src/config.rs --replace-body --text-file new.rs
//! colab edit --symbol main --in main.go --insert-before --text 'func helper() {}'
//! colab edit --in src/config.rs --list            # the file's name paths
//! ```
//!
//! The declaration is found by the language server's `documentSymbol`
//! when one runs (`--lsp`, or a kept server already up for the file), else
//! by tree-sitter (`colab_rewrite::declarations`). A name path is the end
//! of a declaration's full path (`load`, `Config/load`); one that names
//! several is an error that lists them. The edit is a [`ChangeSet`] run
//! through `run_change_set`, so it is previewed, conflict-checked,
//! reported and written (or not) like any other colab run.
//!
//! [`ChangeSet`]: colab_core::fileops::ChangeSet

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Parser;
use colab_core::symbols::{self, Declaration, SymbolEdit};
use colab_core::walker::FileChange;
use colab_core::{BackendRegistry, Error, LanguageBackend, Result, RuleSpec};

use crate::cli::{ApplyFlags, io_to_error};

#[derive(Parser, Debug)]
#[command(group(clap::ArgGroup::new("how").args(["replace_body", "insert_before", "insert_after", "list"]).required(true)))]
pub struct EditArgs {
    /// The declaration's name path: `load`, `Config/load` (`/` between a
    /// container and what it holds).
    #[arg(long, value_name = "NAME PATH", required_unless_present = "list")]
    symbol: Option<String>,

    /// The file it is declared in.
    #[arg(long = "in", value_name = "FILE")]
    in_file: PathBuf,

    /// Replace the whole declaration (doc comments and attributes too)
    /// with the text.
    #[arg(long = "replace-body")]
    replace_body: bool,

    /// Put the text on the lines before the declaration.
    #[arg(long = "insert-before")]
    insert_before: bool,

    /// Put the text on the lines after the declaration.
    #[arg(long = "insert-after")]
    insert_after: bool,

    /// List the file's declarations (their name paths), and exit.
    #[arg(long)]
    list: bool,

    /// The text, re-indented to the declaration's indentation.
    #[arg(long, value_name = "TEXT", conflicts_with = "text_file")]
    text: Option<String>,

    /// Read the text from this file (`-`: stdin).
    #[arg(long = "text-file", value_name = "PATH")]
    text_file: Option<PathBuf>,

    /// Find the declaration with this language server's `documentSymbol`
    /// instead of tree-sitter.
    #[arg(long, value_name = "CMD")]
    lsp: Option<String>,

    /// Seconds for that server to start and index.
    #[arg(long, value_name = "SECS", default_value_t = 120)]
    timeout: u64,

    /// Change to this working directory first.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    change_dir: PathBuf,

    #[command(flatten)]
    apply: ApplyFlags,
}

/// The source backend for `path`: one whose patterns would run on it and
/// that can list declarations.
fn backend_for<'a>(backends: &'a BackendRegistry, path: &Path) -> Option<&'a dyn LanguageBackend> {
    backends
        .languages()
        .into_iter()
        .filter_map(|l| backends.get(l))
        .find(|b| {
            b.declarations("").is_some()
                && b.build_rule(
                    "pattern",
                    RuleSpec::Delete {
                        target: "$X".into(),
                    },
                )
                .is_ok_and(|op| op.is_file_relevant(path))
        })
}

/// The declarations in `file`, and what found them.
pub fn declarations_of(
    backends: &BackendRegistry,
    file: &Path,
    text: &str,
    lsp: Option<&str>,
    timeout: u64,
) -> Result<(Vec<Declaration>, String)> {
    let cwd = std::env::current_dir().map_err(io_to_error)?;
    let absolute = cwd.join(file);
    let server = match lsp {
        Some(command) => Some(colab_lsp_client::ServerCommand::parse(command)?),
        None => kept_server(&absolute, &cwd),
    };
    if let Some(server) = server {
        let root = crate::lsp_commands::root_for(None, &absolute, &cwd)?;
        let decls = colab_lsp_client::symbols::document_symbols(
            &server,
            &root,
            &absolute,
            text,
            Duration::from_secs(timeout),
        )?;
        return Ok((decls, server.program));
    }
    let backend = backend_for(backends, file).ok_or_else(|| {
        Error::UnsupportedOperation(format!(
            "{}: no source language colab knows, and no language server named with --lsp",
            file.display()
        ))
    })?;
    let decls = backend.declarations(text).unwrap_or_default();
    Ok((decls, format!("tree-sitter ({})", backend.lang())))
}

/// The usual server for `file`, if a kept one is already running for it:
/// asking it costs nothing, starting one would.
#[cfg(unix)]
fn kept_server(file: &Path, cwd: &Path) -> Option<colab_lsp_client::ServerCommand> {
    if !colab_lsp_client::daemon::enabled() {
        return None;
    }
    let server = colab_lsp_client::ServerCommand::default_for(file)?;
    let root = crate::lsp_commands::root_for(None, file, cwd).ok()?;
    let socket = colab_lsp_client::daemon::socket_for(&server.program, &server.args, &root);
    std::os::unix::net::UnixStream::connect(socket).ok()?;
    Some(server)
}

#[cfg(not(unix))]
fn kept_server(_: &Path, _: &Path) -> Option<colab_lsp_client::ServerCommand> {
    None
}

fn read_text(args: &EditArgs) -> Result<String> {
    if let Some(text) = &args.text {
        return Ok(text.clone());
    }
    match args.text_file.as_deref() {
        Some(p) if p == Path::new("-") => {
            let mut text = String::new();
            io::stdin().read_to_string(&mut text).map_err(io_to_error)?;
            Ok(text)
        }
        Some(p) => std::fs::read_to_string(p).map_err(|e| Error::io_at(p, e)),
        None => Err(Error::Config(
            "give the text with --text '<…>' or --text-file <path>".into(),
        )),
    }
}

/// The edited file for one symbol edit: shared by `colab edit` and MCP
/// `colab.edit_symbol`. Returns the change and a label for the report.
pub fn plan(
    backends: &BackendRegistry,
    file: &Path,
    name_path: &str,
    edit: SymbolEdit,
    text: &str,
    lsp: Option<&str>,
    timeout: u64,
) -> Result<(FileChange, String)> {
    let source = std::fs::read_to_string(file).map_err(|e| Error::io_at(file, e))?;
    let (decls, by) = declarations_of(backends, file, &source, lsp, timeout)?;
    let found = symbols::resolve(&decls, name_path, &source)
        .map_err(|why| Error::Config(format!("{}: {why}", file.display())))?;
    let after = symbols::apply(&source, found.declaration.range.clone(), edit, text);
    let verb = match edit {
        SymbolEdit::ReplaceBody => "replace",
        SymbolEdit::InsertBefore => "insert before",
        SymbolEdit::InsertAfter => "insert after",
    };
    let label = format!("edit {verb} `{}` (found by {by})", found.path.join("/"));
    Ok((
        FileChange {
            path: file.to_path_buf(),
            before: source,
            after,
            rules_fired: vec![0],
            conflicts: Vec::new(),
        },
        label,
    ))
}

pub fn run(args: EditArgs, backends: &BackendRegistry) -> Result<i32> {
    crate::cli::change_working_dir(&args.change_dir)?;
    let cwd = std::env::current_dir().map_err(io_to_error)?;
    if args.list {
        let source =
            std::fs::read_to_string(&args.in_file).map_err(|e| Error::io_at(&args.in_file, e))?;
        let (decls, by) = declarations_of(
            backends,
            &args.in_file,
            &source,
            args.lsp.as_deref(),
            args.timeout,
        )?;
        let mut out = io::stdout().lock();
        let rows: Vec<serde_json::Value> = symbols::flatten(&decls)
            .iter()
            .map(|f| {
                let line = source[..f.declaration.range.start].matches('\n').count() + 1;
                let end = source[..f.declaration.range.end].matches('\n').count() + 1;
                serde_json::json!({"path": f.path.join("/"), "kind": f.declaration.kind, "line": line, "end_line": end})
            })
            .collect();
        if args.apply.format == crate::format::Format::Human {
            for r in &rows {
                writeln!(
                    out,
                    "{:<40} {:<24} {}-{}",
                    r["path"].as_str().unwrap_or(""),
                    r["kind"].as_str().unwrap_or(""),
                    r["line"],
                    r["end_line"]
                )
                .map_err(io_to_error)?;
            }
        } else {
            writeln!(
                out,
                "{}",
                serde_json::json!({"declarations": rows, "by": by})
            )
            .map_err(io_to_error)?;
        }
        return Ok(0);
    }
    let edit = if args.replace_body {
        SymbolEdit::ReplaceBody
    } else if args.insert_before {
        SymbolEdit::InsertBefore
    } else {
        SymbolEdit::InsertAfter
    };
    let text = read_text(&args)?;
    let started = Instant::now();
    let name_path = args.symbol.clone().unwrap_or_default();
    let (change, label) = plan(
        backends,
        &args.in_file,
        &name_path,
        edit,
        &text,
        args.lsp.as_deref(),
        args.timeout,
    )?;
    crate::cli::run_change_set(
        vec![label],
        colab_core::fileops::ChangeSet::from_edits(vec![change]),
        Vec::new(),
        &args.apply,
        &cwd,
        started,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_backend_is_the_one_whose_patterns_would_run_on_it() {
        let backends = colab_backends::registry();
        assert_eq!(
            backend_for(&backends, Path::new("src/a.rs")).map(|b| b.lang()),
            Some("rust")
        );
        assert!(backend_for(&backends, Path::new("notes.txt")).is_none());
    }
}
