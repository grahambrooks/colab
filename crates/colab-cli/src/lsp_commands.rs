//! The commands that delegate to a language server beyond rename:
//! `colab action`, `colab references`, `colab safe-delete`, `--tidy`, and
//! the MCP tools built on them.
//!
//! The server decides; colab reports. Every change goes through
//! [`run_change_set`](crate::cli::run_change_set) — the same report, diff,
//! `--check`, `--backup`, `--verify`, and git-aware file operations as
//! `rename` and `mv`.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Parser;
use colab_core::fileops::ChangeSet;
use colab_core::{Error, Result};
use colab_lsp_client::ServerCommand;
use colab_lsp_client::actions::{self, Reference, SafeDelete, Select, Target};
use log::info;
use serde_json::{Value, json};

use crate::cli::{ApplyFlags, change_working_dir, io_to_error, project_root, run_change_set};
use crate::format::Format;

const SETTLE: Duration = Duration::from_millis(500);

/// Where in a file a command acts.
#[derive(Debug)]
pub struct Place {
    /// Canonical path.
    pub file: PathBuf,
    /// Byte range; `start == end` for a bare position.
    pub start: usize,
    pub end: usize,
}

/// Resolve `--at FILE:LINE:COL[-LINE:COL]` or `--symbol NAME --in FILE`.
/// A `--symbol` place spans the name, so a code action sees it selected.
pub fn resolve_place(
    at: Option<&str>,
    symbol: Option<&str>,
    in_file: Option<&Path>,
    cwd: &Path,
) -> Result<Place> {
    let read = |file: &Path| fs::read_to_string(file).map_err(|e| Error::io_at(file, e));
    let (file, start, end) = if let Some(at) = at {
        let (head, range_end) = split_range(at);
        let (file, line, column) = parse_position(head).ok_or_else(|| {
            Error::Config(format!(
                "--at wants FILE:LINE:COLUMN or FILE:LINE:COLUMN-LINE:COLUMN, got `{at}`"
            ))
        })?;
        let file = cwd.join(file);
        let text = read(&file)?;
        let offset = |line: usize, column: usize| {
            colab_lsp_client::offset_of_line_column(&text, line, column)
                .ok_or_else(|| Error::Config(format!("{} has no line {line}", file.display())))
        };
        let start = offset(line, column)?;
        let end = match range_end {
            Some((line, column)) => offset(line, column)?,
            None => start,
        };
        if end < start {
            return Err(Error::Config(format!(
                "--at range ends before it starts: `{at}`"
            )));
        }
        (file, start, end)
    } else {
        let name = symbol.ok_or_else(|| Error::Config("pass --at or --symbol".into()))?;
        let file =
            cwd.join(in_file.ok_or_else(|| Error::Config("--symbol needs --in FILE".into()))?);
        let text = read(&file)?;
        let start = colab_lsp_client::first_occurrence(&text, name).ok_or_else(|| {
            Error::Config(format!("`{name}` does not occur in {}", file.display()))
        })?;
        (file, start, start + name.len())
    };
    let file = fs::canonicalize(&file).map_err(|e| Error::io_at(&file, e))?;
    Ok(Place { file, start, end })
}

/// `a.rs:1:2-3:4` → (`a.rs:1:2`, Some((3, 4))). A `-` that is not
/// followed by `LINE:COL` at the very end is part of the path.
fn split_range(at: &str) -> (&str, Option<(usize, usize)>) {
    if let Some((head, tail)) = at.rsplit_once('-')
        && let Some((line, column)) = tail.split_once(':')
        && let (Ok(line), Ok(column)) = (line.parse(), column.parse())
        && parse_position(head).is_some()
    {
        return (head, Some((line, column)));
    }
    (at, None)
}

fn parse_position(at: &str) -> Option<(&str, usize, usize)> {
    let mut parts = at.rsplitn(3, ':');
    let column = parts.next()?.parse().ok()?;
    let line = parts.next()?.parse().ok()?;
    let file = parts.next()?;
    Some((file, line, column))
}

/// `--lsp`, or the default server for `file`'s language.
pub fn server_for(lsp: Option<&str>, file: &Path) -> Result<ServerCommand> {
    match lsp {
        Some(command) => ServerCommand::parse(command),
        None => ServerCommand::default_for(file).ok_or_else(|| {
            Error::Config(format!(
                "no default language server for {}; pass --lsp '<command>'",
                file.display()
            ))
        }),
    }
}

/// `--root`, or the nearest project marker above `file`.
pub fn root_for(root: Option<&Path>, file: &Path, cwd: &Path) -> Result<PathBuf> {
    match root {
        Some(root) => fs::canonicalize(root).map_err(|e| Error::io_at(root, e)),
        None => Ok(project_root(file).unwrap_or_else(|| cwd.to_path_buf())),
    }
}

fn target(place: &Place, root: PathBuf, timeout: u64) -> Target {
    Target {
        file_operations: crate::git::Repo::discover(&root).is_some(),
        root,
        file: place.file.clone(),
        start: place.start,
        end: place.end,
        timeout: Duration::from_secs(timeout),
        settle: SETTLE,
    }
}

// ---------------------------------------------------------------------------
// --tidy
// ---------------------------------------------------------------------------

/// The server `--tidy` asks.
pub struct Tidy<'a> {
    pub server: &'a ServerCommand,
    pub root: &'a Path,
    pub timeout: u64,
}

/// Organize imports and format every edited file in `set`, in place,
/// recording `rule` on each file tidying changed. Returns warnings for
/// steps the server could not do.
pub fn tidy_change_set(tidy: &Tidy<'_>, set: &mut ChangeSet, rule: usize) -> Result<Vec<String>> {
    if set.edits.is_empty() {
        return Ok(Vec::new());
    }
    info!(
        "Tidying {} file(s) via `{}`",
        set.edits.len(),
        tidy.server.display()
    );
    let files: Vec<(PathBuf, String)> = set
        .edits
        .iter()
        .map(|e| (e.path.clone(), e.after.clone()))
        .collect();
    let tidied = actions::tidy(
        tidy.server,
        tidy.root,
        &files,
        Duration::from_secs(tidy.timeout),
        SETTLE,
    )?;
    let mut skipped: Vec<String> = Vec::new();
    for (edit, result) in set.edits.iter_mut().zip(tidied) {
        if result.text != edit.after {
            edit.after = result.text;
            edit.rules_fired.push(rule);
        }
        for step in result.skipped {
            if !skipped.contains(&step) {
                skipped.push(step);
            }
        }
    }
    Ok(skipped
        .into_iter()
        .map(|step| format!("--tidy skipped {step} via `{}`", tidy.server.program))
        .collect())
}

// ---------------------------------------------------------------------------
// colab action
// ---------------------------------------------------------------------------

/// `colab action`.
#[derive(Parser, Debug)]
pub struct ActionArgs {
    /// Where: `FILE:LINE:COL`, or a selection `FILE:LINE:COL-LINE:COL`
    /// (1-based, columns in characters). Extract-style actions need a
    /// selection; most others a position.
    #[arg(long, value_name = "FILE:LINE:COL[-LINE:COL]")]
    at: String,

    /// List the actions the server offers there, and exit.
    #[arg(long, conflicts_with = "title")]
    list: bool,

    /// Pick by kind, a dotted prefix: `refactor.extract` matches
    /// `refactor.extract.function` and `refactor.extract.variable`.
    #[arg(long, value_name = "KIND")]
    kind: Option<String>,

    /// Pick by a case-insensitive substring of the title.
    #[arg(long, value_name = "TEXT")]
    title: Option<String>,

    /// Language-server command; defaults by the file's extension.
    #[arg(long, value_name = "CMD")]
    lsp: Option<String>,

    /// Project root to start the server in.
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Change to this working directory first.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    change_dir: PathBuf,

    /// Seconds to allow for start-up, indexing, and the action.
    #[arg(long, value_name = "SECS", default_value_t = 120)]
    timeout: u64,

    #[command(flatten)]
    apply: ApplyFlags,
}

pub fn run_action(args: ActionArgs) -> Result<i32> {
    change_working_dir(&args.change_dir)?;
    let cwd = std::env::current_dir().map_err(io_to_error)?;
    let place = resolve_place(Some(&args.at), None, None, &cwd)?;
    let server = server_for(args.lsp.as_deref(), &place.file)?;
    let root = root_for(args.root.as_deref(), &place.file, &cwd)?;
    let target = target(&place, root.clone(), args.timeout);
    let started = Instant::now();

    if args.list {
        let list = actions::list_actions(&server, &target, args.kind.as_deref())?;
        let mut out = io::stdout().lock();
        if args.apply.format == Format::Human {
            for a in &list {
                let mut line = format!(
                    "{:<32} {}",
                    a.kind.as_deref().unwrap_or("(command)"),
                    a.title
                );
                if a.preferred {
                    line.push_str("  [preferred]");
                }
                if let Some(reason) = &a.disabled {
                    line.push_str(&format!("  (disabled: {reason})"));
                }
                writeln!(out, "{line}").map_err(io_to_error)?;
            }
            if list.is_empty() {
                writeln!(out, "`{}` offers no code actions here", server.program)
                    .map_err(io_to_error)?;
            }
        } else {
            writeln!(
                out,
                "{}",
                json!({"actions": list.iter().map(action_json).collect::<Vec<_>>()})
            )
            .map_err(io_to_error)?;
        }
        return Ok(0);
    }

    info!(
        "Running a code action via `{}` in {}",
        server.display(),
        root.display()
    );
    let select = Select {
        kind: args.kind.clone(),
        title: args.title.clone(),
    };
    let outcome = actions::run_action(&server, &target, &select)?;
    let label = format!(
        "lsp action `{}` [{}] via {}",
        outcome.title,
        outcome.kind.as_deref().unwrap_or("command"),
        server.program
    );
    let mut warnings = Vec::new();
    if outcome.changes.edits.is_empty() && outcome.changes.ops.is_empty() {
        warnings.push(format!(
            "`{}` returned no edit for this action",
            server.program
        ));
    }
    let tidy = Tidy {
        server: &server,
        root: &root,
        timeout: args.timeout,
    };
    run_change_set(
        &label,
        outcome.changes,
        warnings,
        &args.apply,
        &cwd,
        started,
        Some(tidy),
    )
}

fn action_json(a: &actions::ActionSummary) -> Value {
    let mut v = json!({"title": a.title, "kind": a.kind, "preferred": a.preferred});
    if let Some(reason) = &a.disabled {
        v["disabled"] = json!(reason);
    }
    v
}

// ---------------------------------------------------------------------------
// colab references
// ---------------------------------------------------------------------------

/// `colab references`.
#[derive(Parser, Debug)]
#[command(group(clap::ArgGroup::new("target").required(true).args(["at", "symbol"])))]
pub struct ReferencesArgs {
    /// The symbol's position, `FILE:LINE:COL`.
    #[arg(long, value_name = "FILE:LINE:COL", conflicts_with_all = ["symbol", "in_file"])]
    at: Option<String>,

    /// The symbol's name; its first whole-word occurrence in `--in` is used.
    #[arg(long, value_name = "NAME", requires = "in_file")]
    symbol: Option<String>,

    /// The file `--symbol` is looked up in.
    #[arg(long = "in", value_name = "FILE", id = "in_file")]
    in_file: Option<PathBuf>,

    /// Also list the declaration itself.
    #[arg(long = "include-declaration")]
    include_declaration: bool,

    /// `human` (`path:line:col: line`) or `json`.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,

    /// Report at most this many references (the rest are counted as
    /// `truncated`).
    #[arg(long = "max-results", value_name = "N", default_value_t = colab_core::find::DEFAULT_MAX_RESULTS)]
    max_results: usize,

    #[arg(long, value_name = "CMD")]
    lsp: Option<String>,

    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,

    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    change_dir: PathBuf,

    #[arg(long, value_name = "SECS", default_value_t = 120)]
    timeout: u64,
}

pub fn run_references(args: ReferencesArgs) -> Result<i32> {
    change_working_dir(&args.change_dir)?;
    let cwd = std::env::current_dir().map_err(io_to_error)?;
    let place = resolve_place(
        args.at.as_deref(),
        args.symbol.as_deref(),
        args.in_file.as_deref(),
        &cwd,
    )?;
    let server = server_for(args.lsp.as_deref(), &place.file)?;
    let root = root_for(args.root.as_deref(), &place.file, &cwd)?;
    let found = actions::references(
        &server,
        &target(&place, root, args.timeout),
        args.include_declaration,
    )?;
    let value = references_json(&found, &cwd, args.max_results);
    let mut out = io::stdout().lock();
    if args.format == Format::Human {
        for r in found.iter().take(args.max_results) {
            writeln!(
                out,
                "{}:{}:{}: {}",
                relative(&r.path, &cwd).display(),
                r.at.line,
                r.at.column,
                r.line_text
            )
            .map_err(io_to_error)?;
        }
        let files = value["summary"]["files"].as_u64().unwrap_or(0);
        writeln!(out, "{} reference(s) in {} file(s)", found.len(), files).map_err(io_to_error)?;
    } else {
        writeln!(out, "{value}").map_err(io_to_error)?;
    }
    Ok(0)
}

fn relative<'p>(path: &'p Path, cwd: &Path) -> &'p Path {
    path.strip_prefix(cwd).unwrap_or(path)
}

/// `{"references": [{path, line, col, text}], "summary": {files,
/// references}, "truncated"?}` — the `colab find` shape, with the whole
/// line as `text`.
pub fn references_json(found: &[Reference], cwd: &Path, max: usize) -> Value {
    let mut files: Vec<&Path> = found.iter().map(|r| r.path.as_path()).collect();
    files.dedup();
    let list: Vec<Value> = found
        .iter()
        .take(max)
        .map(|r| {
            json!({
                "path": relative(&r.path, cwd).display().to_string(),
                "line": r.at.line,
                "col": r.at.column,
                "text": r.line_text,
            })
        })
        .collect();
    let mut value = json!({
        "references": list,
        "summary": {"files": files.len(), "references": found.len()},
    });
    if found.len() > max {
        value["truncated"] = json!(found.len() - max);
    }
    value
}

// ---------------------------------------------------------------------------
// colab safe-delete
// ---------------------------------------------------------------------------

/// `colab safe-delete`.
#[derive(Parser, Debug)]
#[command(group(clap::ArgGroup::new("target").required(true).args(["at", "symbol"])))]
pub struct SafeDeleteArgs {
    /// The declaration's name position, `FILE:LINE:COL`.
    #[arg(long, value_name = "FILE:LINE:COL", conflicts_with_all = ["symbol", "in_file"])]
    at: Option<String>,

    /// The symbol's name; its first whole-word occurrence in `--in` (point
    /// it at the declaring file).
    #[arg(long, value_name = "NAME", requires = "in_file")]
    symbol: Option<String>,

    #[arg(long = "in", value_name = "FILE", id = "in_file")]
    in_file: Option<PathBuf>,

    #[arg(long, value_name = "CMD")]
    lsp: Option<String>,

    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,

    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    change_dir: PathBuf,

    #[arg(long, value_name = "SECS", default_value_t = 120)]
    timeout: u64,

    #[command(flatten)]
    apply: ApplyFlags,
}

pub fn run_safe_delete(args: SafeDeleteArgs) -> Result<i32> {
    change_working_dir(&args.change_dir)?;
    let cwd = std::env::current_dir().map_err(io_to_error)?;
    let place = resolve_place(
        args.at.as_deref(),
        args.symbol.as_deref(),
        args.in_file.as_deref(),
        &cwd,
    )?;
    let server = server_for(args.lsp.as_deref(), &place.file)?;
    let root = root_for(args.root.as_deref(), &place.file, &cwd)?;
    let started = Instant::now();
    match actions::safe_delete(&server, &target(&place, root.clone(), args.timeout))? {
        SafeDelete::Unused { symbol, changes } => {
            let label = format!("safe delete `{symbol}` via {}", server.program);
            let tidy = Tidy {
                server: &server,
                root: &root,
                timeout: args.timeout,
            };
            run_change_set(
                &label,
                changes,
                Vec::new(),
                &args.apply,
                &cwd,
                started,
                Some(tidy),
            )
        }
        SafeDelete::Referenced { symbol, references } => {
            let value = references_json(&references, &cwd, colab_core::find::DEFAULT_MAX_RESULTS);
            let mut out = io::stdout().lock();
            if args.apply.format == Format::Human {
                for r in &references {
                    writeln!(
                        out,
                        "{}:{}:{}: {}",
                        relative(&r.path, &cwd).display(),
                        r.at.line,
                        r.at.column,
                        r.line_text
                    )
                    .map_err(io_to_error)?;
                }
            } else {
                let mut value = value;
                value["deleted"] = json!(false);
                value["symbol"] = json!(symbol);
                writeln!(out, "{value}").map_err(io_to_error)?;
            }
            Err(Error::Config(format!(
                "`{symbol}` is still referenced {} time(s) outside its declaration; nothing was deleted",
                references.len()
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// MCP tools
// ---------------------------------------------------------------------------

/// The language-server tools, registered into `colab mcp` by the binary
/// (`colab-mcp` cannot depend on `colab-lsp-client`).
pub struct LspTools;

fn position_properties() -> Value {
    json!({
        "at": {"type": "string", "description": "`FILE:LINE:COL` (1-based, columns in characters), relative to `cwd`."},
        "symbol": {"type": "string", "description": "Instead of `at`: a name; its first whole-word occurrence in `in` is used."},
        "in": {"type": "string", "description": "The file `symbol` is looked up in — point it at the declaring file."},
        "lsp": {"type": "string", "description": "Language-server command; defaults by file extension (rust-analyzer, gopls, clangd, …)."},
        "root": {"type": "string", "description": "Project root for the server; defaults to the nearest project marker."},
        "cwd": {"type": "string", "description": "Directory paths are relative to; defaults to the server's."},
        "timeout": {"type": "integer", "description": "Seconds for start-up, indexing, and the request (default 120)."}
    })
}

fn write_properties(props: &mut Value) {
    props["write"] = json!({"type": "boolean", "default": false, "description": "Apply the change. Default: preview only. File operations (moves) need a git work tree with clean touched paths."});
    props["tidy"] = json!({"type": "boolean", "default": false, "description": "Organize imports and format changed files via the server, reported as its own rule."});
    props["detail"] =
        json!({"type": "string", "enum": ["summary", "counts", "diff"], "default": "counts"});
}

impl colab_mcp::ToolProvider for LspTools {
    fn tools(&self) -> Vec<Value> {
        let mut references = position_properties();
        references["include_declaration"] = json!({"type": "boolean", "default": false});
        references["max_results"] =
            json!({"type": "integer", "default": colab_core::find::DEFAULT_MAX_RESULTS});

        let mut rename = position_properties();
        rename["to"] = json!({"type": "string", "description": "The new name."});
        write_properties(&mut rename);

        let mut action = position_properties();
        action["at"] = json!({"type": "string", "description": "`FILE:LINE:COL` or a selection `FILE:LINE:COL-LINE:COL`, relative to `cwd`. Extract-style actions need a selection."});
        action.as_object_mut().unwrap().remove("symbol");
        action.as_object_mut().unwrap().remove("in");
        action["list"] = json!({"type": "boolean", "default": false, "description": "List the actions offered at `at` instead of running one."});
        action["kind"] = json!({"type": "string", "description": "Dotted-prefix kind, e.g. `refactor.extract` or `refactor.inline`."});
        action["title"] = json!({"type": "string", "description": "Case-insensitive substring of the action's title."});
        write_properties(&mut action);

        vec![
            json!({
                "name": "colab.references",
                "description": "Find every reference to the symbol at a position, as resolved by the language's own server (scope-aware, unlike text search). Read-only. Use before changing or deleting a symbol to see its reach.",
                "annotations": {"readOnlyHint": true},
                "inputSchema": {"type": "object", "properties": references, "additionalProperties": false}
            }),
            json!({
                "name": "colab.rename",
                "description": "Scope-aware rename via the language's server (rust-analyzer, gopls, clangd, …): renames the symbol at a position and every reference the server resolves to it — not other symbols that share the name. Returns the same report as colab.preview; set `write` to apply.",
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false},
                "inputSchema": {"type": "object", "properties": rename, "required": ["to"], "additionalProperties": false}
            }),
            json!({
                "name": "colab.action",
                "description": "Run one of the language server's code actions — extract function/variable, inline, change signature, organize imports, … — at a position or selection. Call with `list: true` first to see what is offered, then pick with `kind` and/or `title`. Returns a colab report; set `write` to apply.",
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false},
                "inputSchema": {"type": "object", "properties": action, "required": ["at"], "additionalProperties": false}
            }),
        ]
    }

    fn call(&self, name: &str, args: &Value) -> Option<Result<Value>> {
        let run = match name {
            "colab.references" => mcp_references,
            "colab.rename" => mcp_rename,
            "colab.action" => mcp_action,
            _ => return None,
        };
        Some(run(args))
    }
}

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// `cwd`, place, server, root, and timeout from tool arguments.
fn mcp_context(args: &Value) -> Result<(PathBuf, Place, ServerCommand, PathBuf, u64)> {
    let cwd = match arg(args, "cwd") {
        Some(dir) => fs::canonicalize(dir).map_err(|e| Error::io_at(Path::new(dir), e))?,
        None => std::env::current_dir().map_err(io_to_error)?,
    };
    let place = resolve_place(
        arg(args, "at"),
        arg(args, "symbol"),
        arg(args, "in").map(Path::new),
        &cwd,
    )?;
    let server = server_for(arg(args, "lsp"), &place.file)?;
    let root = root_for(
        arg(args, "root")
            .map(Path::new)
            .map(|r| cwd.join(r))
            .as_deref(),
        &place.file,
        &cwd,
    )?;
    let timeout = args.get("timeout").and_then(Value::as_u64).unwrap_or(120);
    Ok((cwd, place, server, root, timeout))
}

fn mcp_references(args: &Value) -> Result<Value> {
    let (cwd, place, server, root, timeout) = mcp_context(args)?;
    let found = actions::references(
        &server,
        &target(&place, root, timeout),
        flag(args, "include_declaration"),
    )?;
    let max = args
        .get("max_results")
        .and_then(Value::as_u64)
        .map_or(colab_core::find::DEFAULT_MAX_RESULTS, |n| n as usize);
    Ok(references_json(&found, &cwd, max))
}

fn mcp_rename(args: &Value) -> Result<Value> {
    let (cwd, place, server, root, timeout) = mcp_context(args)?;
    let to = arg(args, "to").ok_or_else(|| Error::Config("`to` is required".into()))?;
    let outcome = colab_lsp_client::rename(
        &server,
        &colab_lsp_client::RenameRequest {
            file_operations: crate::git::Repo::discover(&root).is_some(),
            root: root.clone(),
            file: place.file.clone(),
            offset: place.start,
            new_name: to.to_string(),
            timeout: Duration::from_secs(timeout),
            settle: SETTLE,
        },
    )?;
    let label = format!(
        "lsp rename `{}` -> `{to}` via {}",
        outcome.old_name.as_deref().unwrap_or("?"),
        server.program
    );
    let tidy = Tidy {
        server: &server,
        root: &root,
        timeout,
    };
    change_set_json(&label, outcome.changes, Vec::new(), args, &cwd, Some(tidy))
}

fn mcp_action(args: &Value) -> Result<Value> {
    let (cwd, place, server, root, timeout) = mcp_context(args)?;
    let target = target(&place, root.clone(), timeout);
    if flag(args, "list") {
        let list = actions::list_actions(&server, &target, arg(args, "kind"))?;
        return Ok(json!({"actions": list.iter().map(action_json).collect::<Vec<_>>()}));
    }
    let select = Select {
        kind: arg(args, "kind").map(str::to_string),
        title: arg(args, "title").map(str::to_string),
    };
    let outcome = actions::run_action(&server, &target, &select)?;
    let label = format!(
        "lsp action `{}` [{}] via {}",
        outcome.title,
        outcome.kind.as_deref().unwrap_or("command"),
        server.program
    );
    let tidy = Tidy {
        server: &server,
        root: &root,
        timeout,
    };
    change_set_json(&label, outcome.changes, Vec::new(), args, &cwd, Some(tidy))
}

/// The `colab.preview`-shaped report for a change set — the same JSON as
/// `--format json` — applying it first when `write` is set. Nothing is
/// printed: stdout is the MCP transport.
fn change_set_json(
    label: &str,
    mut set: ChangeSet,
    mut warnings: Vec<String>,
    args: &Value,
    cwd: &Path,
    tidy: Option<Tidy<'_>>,
) -> Result<Value> {
    use colab_core::render::{self, ChangeCollector, Detail, RenderOptions};

    let mut labels = vec![label.to_string()];
    if flag(args, "tidy")
        && let Some(tidy) = &tidy
    {
        labels.push(format!("tidy via {}", tidy.server.program));
        warnings.extend(tidy_change_set(tidy, &mut set, 1)?);
    }
    let options = crate::apply::ApplyOptions::default();
    let prepared = crate::apply::prepare(&set, &options)?;
    warnings.extend(prepared.warnings.iter().cloned());
    let write = flag(args, "write");
    if write {
        crate::apply::apply(&set, &prepared, &options, |_, _| Ok(()))?;
    }

    let detail = arg(args, "detail")
        .and_then(Detail::parse)
        .unwrap_or(Detail::Counts);
    let mut collector = ChangeCollector::new(RenderOptions {
        detail,
        max_files: render::DEFAULT_MAX_FILES,
        max_diff_bytes: render::DEFAULT_MAX_DIFF_BYTES,
    });
    let mut report = colab_core::RunReport::with_rules(labels);
    let rel = |p: &Path| relative(p, cwd).to_path_buf();
    report.files_visited = set.edits.len() as u64;
    report.file_ops = set.ops.iter().map(|op| op.map_paths(rel)).collect();
    report.warnings = warnings;
    for edit in &set.edits {
        let mut change = edit.clone();
        change.path = rel(&change.path);
        report.record(&change);
        collector.push(&change);
    }
    if report.rules[0].files_matched == 0 && !set.ops.is_empty() {
        report.rules[0].files_matched = set.ops.len() as u64;
    }
    let mut value = collector.to_json(&report);
    let advisories = render::advisories(&report);
    if !advisories.is_empty() {
        value["warnings"] = json!(advisories);
    }
    value["written"] = json!(write);
    Ok(value)
}

// ---------------------------------------------------------------------------
// --verify-lsp
// ---------------------------------------------------------------------------

/// `--verify-lsp`: ask each changed file's language server whether the
/// change adds errors, before anything is written, and record each added
/// error as a conflict of the whole file. `command` overrides the server
/// chosen by extension. A file with no default server is noted, not
/// checked.
pub fn verify_lsp(
    command: Option<&str>,
    changes: &[colab_core::FileChange],
    report: &mut colab_core::RunReport,
) -> Result<()> {
    use colab_lsp_client::diagnostics::{Changed, added_errors};
    use std::collections::BTreeMap;

    let cwd = std::env::current_dir().map_err(io_to_error)?;
    // (server, root) -> files; and each canonical path's name as reported.
    let mut groups: BTreeMap<(String, PathBuf), (ServerCommand, Vec<Changed>)> = BTreeMap::new();
    let mut shown: std::collections::HashMap<PathBuf, PathBuf> = Default::default();
    for change in changes.iter().filter(|c| c.changed()) {
        let server = match command {
            Some(command) => ServerCommand::parse(command)?,
            None => match ServerCommand::default_for(&change.path) {
                Some(server) => server,
                None => {
                    report.warnings.push(format!(
                        "--verify-lsp: no default language server for {}; not checked (pass --verify-lsp='<command>')",
                        change.path.display()
                    ));
                    continue;
                }
            },
        };
        let path = fs::canonicalize(&change.path).map_err(|e| Error::io_at(&change.path, e))?;
        let root = root_for(None, &path, &cwd)?;
        shown.insert(path.clone(), change.path.clone());
        groups
            .entry((server.display(), root))
            .or_insert_with(|| (server, Vec::new()))
            .1
            .push(Changed {
                path,
                before: change.before.clone(),
                after: change.after.clone(),
            });
    }
    for ((name, root), (server, files)) in groups {
        info!(
            "--verify-lsp: asking `{name}` about {} file(s) in {}",
            files.len(),
            root.display()
        );
        for added in added_errors(&server, &root, Duration::from_secs(120), SETTLE, &files)? {
            let path = shown.get(&added.path).cloned().unwrap_or(added.path);
            for error in added.errors {
                report.conflicts.push(colab_core::Conflict {
                    path: path.clone(),
                    rule: None,
                    message: format!("`{name}` reports a new error at {error}"),
                });
            }
        }
    }
    Ok(())
}
