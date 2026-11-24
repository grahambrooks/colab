//! The commands that delegate to a language server beyond rename:
//! `colab action`, `colab references`, `colab safe-delete`, `--tidy`, and
//! the MCP tools built on them.
//!
//! The server decides; colab reports. Every change goes through
//! [`run_change_set`] — the same report, diff,
//! `--check`, `--backup`, `--verify`, and git-aware file operations as
//! `rename` and `mv`.
use std::fmt::Write as _;

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
// The parts in their own files: `--tidy`, `rename --include-text`, move
// planning, the MCP tools and `--verify-lsp`.
// ---------------------------------------------------------------------------

mod mcp;
mod moves;
mod text;
mod tidy;
mod verify;

pub use mcp::LspTools;
pub(crate) use moves::{MoveSpec, plan_move};
pub(crate) use text::{merge_text, text_occurrences, text_summary};
pub(crate) use tidy::{Tidy, tidy_change_set, tidy_pending};
pub(crate) use verify::verify_lsp;

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

pub fn run_action(args: ActionArgs) -> Result<u8> {
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
                    let _ = write!(line, "  (disabled: {reason})");
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
    let tidy = tidy::Tidy {
        server: &server,
        root: &root,
        timeout: args.timeout,
    };
    run_change_set(
        vec![label.to_string()],
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

pub fn run_references(args: ReferencesArgs) -> Result<u8> {
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
    print_locations(
        "references",
        "reference(s)",
        &found,
        &cwd,
        args.max_results,
        args.format,
    )
}

/// `path:line:col: line` per place and a count, or the JSON document.
fn print_locations(
    key: &str,
    noun: &str,
    found: &[Reference],
    cwd: &Path,
    max: usize,
    format: Format,
) -> Result<u8> {
    let value = locations_json(key, found, cwd, max);
    let mut out = io::stdout().lock();
    if format == Format::Human {
        for r in found.iter().take(max) {
            writeln!(
                out,
                "{}:{}:{}: {}",
                relative(&r.path, cwd).display(),
                r.at.line,
                r.at.column,
                r.line_text
            )
            .map_err(io_to_error)?;
        }
        let files = value["summary"]["files"].as_u64().unwrap_or(0);
        writeln!(out, "{} {noun} in {files} file(s)", found.len()).map_err(io_to_error)?;
    } else {
        writeln!(out, "{value}").map_err(io_to_error)?;
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// colab calls / colab implementations
// ---------------------------------------------------------------------------

/// `colab calls`.
#[derive(Parser, Debug)]
#[command(group(clap::ArgGroup::new("target").required(true).args(["at", "symbol"])))]
pub struct CallsArgs {
    /// The function's position, `FILE:LINE:COL`.
    #[arg(long, value_name = "FILE:LINE:COL", conflicts_with_all = ["symbol", "in_file"])]
    at: Option<String>,

    /// The function's name; its first whole-word occurrence in `--in` is used.
    #[arg(long, value_name = "NAME", requires = "in_file")]
    symbol: Option<String>,

    /// The file `--symbol` is looked up in.
    #[arg(long = "in", value_name = "FILE", id = "in_file")]
    in_file: Option<PathBuf>,

    /// What the function calls, instead of who calls it.
    #[arg(long)]
    outgoing: bool,

    /// `human` (`path:line:col: line`) or `json`.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,

    /// Report at most this many functions (the rest are counted as
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

pub fn run_calls(args: CallsArgs) -> Result<u8> {
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
    let found = actions::calls(&server, &target(&place, root, args.timeout), args.outgoing)?;
    let (key, noun) = if args.outgoing {
        ("callees", "callee(s)")
    } else {
        ("callers", "caller(s)")
    };
    print_locations(key, noun, &found, &cwd, args.max_results, args.format)
}

/// `colab implementations`.
#[derive(Parser, Debug)]
#[command(group(clap::ArgGroup::new("target").required(true).args(["at", "symbol"])))]
pub struct ImplementationsArgs {
    /// The interface's, trait's or method's position, `FILE:LINE:COL`.
    #[arg(long, value_name = "FILE:LINE:COL", conflicts_with_all = ["symbol", "in_file"])]
    at: Option<String>,

    /// Its name; the first whole-word occurrence in `--in` is used.
    #[arg(long, value_name = "NAME", requires = "in_file")]
    symbol: Option<String>,

    /// The file `--symbol` is looked up in.
    #[arg(long = "in", value_name = "FILE", id = "in_file")]
    in_file: Option<PathBuf>,

    /// `human` (`path:line:col: line`) or `json`.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,

    /// Report at most this many implementations (the rest are counted as
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

pub fn run_implementations(args: ImplementationsArgs) -> Result<u8> {
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
    let found = actions::implementations(&server, &target(&place, root, args.timeout))?;
    print_locations(
        "implementations",
        "implementation(s)",
        &found,
        &cwd,
        args.max_results,
        args.format,
    )
}

fn relative<'p>(path: &'p Path, cwd: &Path) -> &'p Path {
    path.strip_prefix(cwd).unwrap_or(path)
}

/// `{"references": [{path, line, col, text}], "summary": {files,
/// references}, "truncated"?}` — the `colab find` shape, with the whole
/// line as `text`.
pub fn references_json(found: &[Reference], cwd: &Path, max: usize) -> Value {
    locations_json("references", found, cwd, max)
}

/// [`references_json`]'s shape under another `key` (`callers`,
/// `callees`, `implementations`), the count under the same key.
pub fn locations_json(key: &str, found: &[Reference], cwd: &Path, max: usize) -> Value {
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
        key: list,
        "summary": {"files": files.len(), key: found.len()},
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

pub fn run_safe_delete(args: SafeDeleteArgs) -> Result<u8> {
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
            let tidy = tidy::Tidy {
                server: &server,
                root: &root,
                timeout: args.timeout,
            };
            run_change_set(
                vec![label.to_string()],
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
