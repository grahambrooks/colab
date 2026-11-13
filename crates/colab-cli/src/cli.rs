//! Command-line interface for the colab binary.
//!
//! Parses arguments via `clap`, dispatches to per-subcommand
//! handlers, and returns the success exit code (0 or 10) to `main`.
//! Errors bubble up via [`colab_core::Error`] and `main` maps them to
//! the documented exit-code table.

use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser};
use log::info;

use std::time::Instant;

use colab_core::render::{self, Detail, RenderOptions};
use colab_core::report::RunReport;
use colab_core::walker::{self, FileChange, WalkOptions, WalkOutcome};
use colab_core::{BackendRegistry, CodeTransformer, Error, Result};
use colab_dsl as codemod;

use crate::discover;
use crate::format::{self, ExecMode, Format};
use crate::language_server;

static VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    ".",
    include_str!(concat!(env!("OUT_DIR"), "/version.txt"))
);

/// The registry of language backends compiled into this binary.
///
/// Registration lives in `colab-backends` so the binary, the corpus
/// harness, and the MCP tests cannot disagree about which backends exist.
fn default_backends() -> BackendRegistry {
    colab_backends::registry()
}

#[derive(Parser, Debug)]
#[command(
    color = clap::ColorChoice::Auto,
    author = "Graham Brooks",
    version = VERSION,
    about = "Scripted, AST-aware code refactoring",
    long_about = r#"
Code Lab (colab) is a command-line code refactoring (or 'codemod') tool.

It reads a small DSL describing one or more refactoring rules, parses
target source files with tree-sitter, and rewrites them. Use `colab
refactor` to run a script, `colab schema` / `colab list-languages` /
`colab list-rules <lang>` for capability discovery, `colab explain
--script foo.codemod` to inspect a parsed script as JSON, `colab
server` for the LSP, or `colab mcp` for the Model Context Protocol
server (so MCP-aware hosts can call preview / apply / schema /
lint_script as tools).

Exit codes:
   0   Success (no changes needed, or --write succeeded).
   1   Generic / configuration error.
   2   Script parse error.
   3   Unsupported namespace or operation.
   4   I/O error.
  10   --check found changes that would be made.
"#
)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Print this command's flags, subcommands, and constraints as JSON
    /// (for agents and scripts), then exit. Works after any subcommand:
    /// `colab refactor --help-json`.
    #[arg(long = "help-json", global = true)]
    help_json: bool,
}

#[derive(Parser, Debug)]
#[allow(clippy::large_enum_variant)] // clap derive shape; not on a hot path
enum Commands {
    /// Run a codemod script against one or more paths.
    Refactor(RefactorArgs),
    /// Start the colab language server over stdio.
    Server(ServerArgs),
    /// Start the colab MCP server (Model Context Protocol) over stdio.
    Mcp,
    /// Print the JSON capability schema for every registered backend.
    Schema,
    /// Search: report where each rule matches, without changing anything.
    Find(FindArgs),
    /// Scope-aware rename, delegated to the language's own server
    /// (rust-analyzer, gopls, clangd, …) and reported like a codemod.
    Rename(RenameArgs),
    /// Move a file or directory with `git mv`, letting the language server
    /// update what refers to it.
    Mv(MvArgs),
    /// Print the syntax tree of a snippet or a pattern, to see why a
    /// pattern does or does not match.
    Ast(AstArgs),
    /// Parse a script and print its IR as JSON without running it.
    Explain(ExplainArgs),
    /// List every language backend the binary knows about.
    ListLanguages,
    /// List the modules and actions a backend supports.
    ListRules(ListRulesArgs),
    /// Manage discoverable codemod packs.
    Pack(PackArgs),
    /// Restore files from a `--backup <dir>` produced by a prior
    /// `colab refactor`, `rename`, or `mv` run, undoing its file
    /// operations (through git) first.
    Undo(UndoArgs),
}

#[derive(Parser, Debug)]
struct UndoArgs {
    /// Backup directory to restore from.
    #[arg(long = "from", value_name = "DIR", required = true)]
    from: PathBuf,
}

#[derive(Parser, Debug)]
struct PackArgs {
    #[command(subcommand)]
    cmd: PackCommand,
}

#[derive(Parser, Debug)]
enum PackCommand {
    /// List discoverable packs in `<repo>/.colab/packs/` and
    /// `~/.colab/packs/`.
    List,
}

#[derive(Parser, Debug)]
struct ServerArgs {
    /// Reserved for future TCP transport; currently informational only.
    #[arg(long, default_value_t = 8080)]
    port: u16,
}

#[derive(Parser, Debug)]
struct RefactorArgs {
    #[command(flatten)]
    script: ScriptArgs,

    /// Change to this working directory before resolving paths.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    change_dir: PathBuf,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,

    /// Apply changes in place. Default for `--format human` on a TTY.
    #[arg(long, conflicts_with_all = ["dry_run", "check"])]
    write: bool,

    /// Report what would change without writing. Default for
    /// `--format json|ndjson|diff` and for non-TTY stdout.
    #[arg(long = "dry-run", conflicts_with_all = ["write", "check"])]
    dry_run: bool,

    /// Exit 10 if any file would change. Implies `--dry-run`.
    #[arg(long, conflicts_with_all = ["write", "dry_run"])]
    check: bool,

    /// Read source from stdin instead of walking paths. Pair with
    /// `--path` to give a filename hint for language detection.
    #[arg(long, requires = "stdin_path")]
    stdin: bool,

    /// Path hint for `--stdin`. Drives is-this-relevant routing.
    #[arg(long = "path", value_name = "PATH", id = "stdin_path")]
    stdin_path: Option<PathBuf>,

    /// Glob pattern to include. Repeatable. If any include is set,
    /// only matching files are processed. Uses gitignore syntax.
    #[arg(long = "include", value_name = "GLOB")]
    include: Vec<String>,

    /// Glob pattern to exclude. Repeatable. Applied after `--include`.
    #[arg(long = "exclude", value_name = "GLOB")]
    exclude: Vec<String>,

    /// Don't honour `.gitignore` / hidden-file rules. By default
    /// the walker behaves like `git ls-files`.
    #[arg(long = "no-ignore")]
    no_ignore: bool,

    /// Restrict the run to files changed since the given git ref
    /// (e.g. `main`). Bypasses tree walking; iterates the git
    /// diff directly.
    #[arg(long = "changed-since", value_name = "REF")]
    changed_since: Option<String>,

    /// Restrict the run to files in the git index (i.e. `git add`-ed).
    /// Mutually exclusive with `--changed-since`.
    #[arg(long, conflicts_with = "changed_since")]
    staged: bool,

    /// Worker thread count. Defaults to `num_cpus`. Falls back to
    /// the `COLAB_JOBS` env var when unset. Set to 1 to force
    /// sequential processing.
    #[arg(long, value_name = "N")]
    jobs: Option<usize>,

    /// Run this shell command after each rule's edits. A non-zero
    /// exit reverts that rule's changes and aborts the run.
    /// Implies `--write`.
    #[arg(long, value_name = "CMD", conflicts_with_all = ["dry_run", "check"])]
    verify: Option<String>,

    /// After each successful rule, `git add -u && git commit -m
    /// "colab: <rule>"`. Requires a git repo. Implies `--write`.
    #[arg(long = "commit-per-rule", conflicts_with_all = ["dry_run", "check"])]
    commit_per_rule: bool,

    /// After applying every rule, run this shell command. On
    /// failure, binary-search the rule list to identify the single
    /// breaking rule, then revert the working tree to its pre-run
    /// state and exit non-zero. Conflicts with `--verify`.
    #[arg(long, value_name = "CMD",
          conflicts_with_all = ["verify", "commit_per_rule", "dry_run", "check"])]
    bisect: Option<String>,

    /// Before any rule writes a file, save the pre-modification
    /// contents to `<dir>/<rel-path>`. Pair with `colab undo
    /// --from <dir>` to roll back the run.
    #[arg(long, value_name = "DIR", conflicts_with_all = ["dry_run", "check"])]
    backup: Option<PathBuf>,

    /// How much detail to report: `summary` (aggregate counters only),
    /// `counts` (adds per-rule match counts and changed paths), or
    /// `diff` (adds capped unified diffs). Defaults to `counts`, or
    /// `diff` for `--format diff`.
    #[arg(long, value_name = "LEVEL", value_parser = parse_detail)]
    detail: Option<Detail>,

    /// Maximum number of per-file diffs to render at `--detail diff`.
    #[arg(long = "max-files", value_name = "N",
          default_value_t = render::DEFAULT_MAX_FILES)]
    max_files: usize,

    /// Maximum size of any single rendered diff, in bytes.
    #[arg(long = "max-diff-bytes", value_name = "N",
          default_value_t = render::DEFAULT_MAX_DIFF_BYTES)]
    max_diff_bytes: usize,

    /// Suppress per-file events; emit only the final aggregate
    /// summary. Alias for `--detail summary`.
    #[arg(long = "summary-only", conflicts_with = "detail")]
    summary_only: bool,

    /// Files or directories to process. Defaults to the (possibly
    /// changed) working directory if none are supplied. Ignored with
    /// `--stdin`.
    paths: Vec<PathBuf>,
}

#[derive(Parser, Debug)]
struct FindArgs {
    #[command(flatten)]
    script: ScriptArgs,

    /// Change to this working directory before resolving paths.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    change_dir: PathBuf,

    /// `human`: `path:line:col: text` lines. `json`: one document.
    #[arg(long, value_enum, default_value_t = FindFormat::Human)]
    format: FindFormat,

    /// `concise`: the first line of each match. `detailed`: adds
    /// metavariable captures and the full (capped) match text.
    #[arg(long, value_enum, default_value_t = FindDetailArg::Concise)]
    detail: FindDetailArg,

    /// List at most this many matches; the rest are counted.
    #[arg(long = "max-results", value_name = "N", default_value_t = colab_core::find::DEFAULT_MAX_RESULTS)]
    max_results: usize,

    /// Whitelist files via gitignore-syntax glob. Repeatable.
    #[arg(long = "include", value_name = "GLOB")]
    include: Vec<String>,

    /// Blacklist files via gitignore-syntax glob. Repeatable.
    #[arg(long = "exclude", value_name = "GLOB")]
    exclude: Vec<String>,

    /// Don't honour `.gitignore` or hidden-file rules.
    #[arg(long = "no-ignore")]
    no_ignore: bool,

    /// Files or directories to search. Defaults to `.`.
    paths: Vec<PathBuf>,
}

#[derive(Parser, Debug)]
#[command(group(clap::ArgGroup::new("target").required(true).args(["at", "symbol"])))]
struct RenameArgs {
    /// The symbol's position: `FILE:LINE:COLUMN`, 1-based, column in
    /// characters. Exact; prefer it when you know where the definition is.
    #[arg(long, value_name = "FILE:LINE:COL", conflicts_with_all = ["symbol", "in_file"])]
    at: Option<String>,

    /// The symbol's current name. Its first whole-word occurrence in
    /// `--in` is renamed — with every reference the server resolves to it.
    #[arg(long, value_name = "NAME", requires = "in_file")]
    symbol: Option<String>,

    /// The file `--symbol` is looked up in.
    #[arg(long = "in", value_name = "FILE", id = "in_file")]
    in_file: Option<PathBuf>,

    /// The new name.
    #[arg(long, value_name = "NAME")]
    to: String,

    /// Language-server command, e.g. `"typescript-language-server --stdio"`.
    /// Defaults by file extension (rust-analyzer, gopls, clangd, …).
    #[arg(long, value_name = "CMD")]
    lsp: Option<String>,

    /// Project root to start the server in. Defaults to the nearest
    /// ancestor of the file holding a project marker (`.git`,
    /// `Cargo.toml`, `go.mod`, `package.json`, …).
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Change to this working directory first.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    change_dir: PathBuf,

    /// Seconds to allow for start-up, indexing, and the rename.
    #[arg(long, value_name = "SECS", default_value_t = 120)]
    timeout: u64,

    #[command(flatten)]
    apply: ApplyFlags,
}

/// Output and write flags shared by the commands that apply a language
/// server's answer (`rename`, `mv`).
#[derive(clap::Args, Debug)]
struct ApplyFlags {
    /// Output format, as for `refactor`.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,

    /// Apply the edits in place.
    #[arg(long, conflicts_with_all = ["dry_run", "check"])]
    write: bool,

    /// Report what would change without writing.
    #[arg(long = "dry-run", conflicts_with_all = ["write", "check"])]
    dry_run: bool,

    /// Like `--dry-run`, but exit 10 if anything would change.
    #[arg(long, conflicts_with_all = ["write", "dry_run"])]
    check: bool,

    /// Save each file's pre-change contents (and the file operations
    /// made) under this directory; `colab undo --from DIR` reverses both.
    #[arg(long, value_name = "DIR", conflicts_with_all = ["dry_run", "check"])]
    backup: Option<PathBuf>,

    /// After writing, run this shell command; on failure undo every file
    /// operation and edit, and exit 1. Implies `--write`.
    #[arg(long, value_name = "CMD", conflicts_with_all = ["dry_run", "check"])]
    verify: Option<String>,

    /// Make file operations even if a path they touch has uncommitted
    /// changes. Without it such a run is refused, so undoing colab can
    /// never discard work of yours.
    #[arg(long = "allow-dirty")]
    allow_dirty: bool,

    /// Commit the file operations on their own (`colab: move a -> b`)
    /// before writing any edit, so git pairs each moved file with its old
    /// path and `git log --follow` keeps working. Only those paths are
    /// committed; anything else you have staged stays staged.
    #[arg(long = "commit-moves")]
    commit_moves: bool,

    /// `summary`, `counts`, or `diff`, as for `refactor`.
    #[arg(long, value_name = "LEVEL", value_parser = parse_detail)]
    detail: Option<Detail>,
}

/// `colab mv FROM TO`.
#[derive(Parser, Debug)]
struct MvArgs {
    /// The file or directory to move.
    from: PathBuf,

    /// Where it goes. An existing directory means "into it", as for `mv`.
    to: PathBuf,

    /// Language-server command that updates what refers to the moved file
    /// (`workspace/willRenameFiles`). Defaults by the file's extension.
    #[arg(long, value_name = "CMD", conflicts_with = "no_lsp")]
    lsp: Option<String>,

    /// Move with git only; update no references.
    #[arg(long = "no-lsp")]
    no_lsp: bool,

    /// Project root to start the server in (default: nearest project
    /// marker above FROM).
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Change to this working directory first.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    change_dir: PathBuf,

    /// Seconds to allow for start-up, indexing, and the server's answer.
    #[arg(long, value_name = "SECS", default_value_t = 120)]
    timeout: u64,

    #[command(flatten)]
    apply: ApplyFlags,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum FindFormat {
    Human,
    Json,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum FindDetailArg {
    Concise,
    Detailed,
}

#[derive(Parser, Debug)]
struct AstArgs {
    /// Language backend whose grammar to use, e.g. `go`.
    #[arg(long)]
    lang: String,

    /// Show how this pattern parses (metavariables included), instead
    /// of a snippet.
    #[arg(long, conflicts_with = "stdin")]
    pattern: Option<String>,

    /// Read the snippet to parse from stdin.
    #[arg(long)]
    stdin: bool,

    /// `human`: the tree as indented text. `json`: one document.
    #[arg(long, value_enum, default_value_t = FindFormat::Human)]
    format: FindFormat,
}

#[derive(Parser, Debug)]
struct ExplainArgs {
    #[command(flatten)]
    script: ScriptArgs,
}

/// Where the codemod script comes from: a file, stdin, or inline rules.
///
/// Exactly one of `--script` / `--rule` is required. Inline rules exist
/// so an agent (or a one-off shell command) can run a rule without
/// first writing a `.codemod` file.
#[derive(clap::Args, Debug)]
#[group(required = true, multiple = false)]
struct ScriptArgs {
    /// Path to the codemod script. `-` reads the script from stdin.
    #[arg(long = "script", value_name = "PATH")]
    script_path: Option<PathBuf>,

    /// An inline rule, e.g. `match go::import "a" { replace "b" }`.
    /// Repeatable; rules run in the order given. A single value that is
    /// a whole `refactor "…" { … }` script is used as-is.
    #[arg(short = 'e', long = "rule", value_name = "RULE")]
    rules: Vec<String>,
}

impl ScriptArgs {
    /// `true` when the script itself is read from stdin (`--script -`).
    fn reads_stdin(&self) -> bool {
        self.script_path.as_deref() == Some(Path::new("-"))
    }

    /// The script text, plus the path `include "..."` resolves against.
    fn load(&self) -> Result<(String, PathBuf)> {
        let cwd = std::env::current_dir().map_err(io_to_error)?;
        match &self.script_path {
            Some(_) if self.reads_stdin() => {
                let mut text = String::new();
                io::stdin().read_to_string(&mut text).map_err(io_to_error)?;
                Ok((text, cwd.join("<stdin>")))
            }
            Some(path) => {
                let text = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
                Ok((text, path.clone()))
            }
            None => Ok((inline_script(&self.rules), cwd.join("<inline>"))),
        }
    }

    fn compile(&self, backends: &BackendRegistry) -> Result<codemod::Refactoring> {
        let (text, base) = self.load()?;
        codemod::compile_at_source(&text, &base, backends).map_err(|e| self.relocate(&text, e))
    }

    /// Re-anchor a parse error from the synthesised inline wrapper onto
    /// the rules the user actually typed, so line, column, and caret
    /// snippet all point into their `-e` text rather than the wrapper.
    fn relocate(&self, text: &str, err: Error) -> Error {
        let Error::Parse(detail) = err else {
            return err;
        };
        let rules = self.rules.join("\n");
        match text.strip_prefix(INLINE_PREFIX) {
            Some(_) if self.script_path.is_none() => {
                let offset = detail.offset.saturating_sub(INLINE_PREFIX.len());
                Error::Parse(colab_core::ParseDetail::at_offset(
                    &rules,
                    offset,
                    detail.message,
                    detail.expected,
                ))
            }
            _ => Error::Parse(detail),
        }
    }
}

/// Wrap inline `-e` rules in a `refactor` block.
///
/// The wrapper opens on the first rule's line so parse errors keep the
/// line number the user sees in their own `-e` list (rule N is line N
/// when each rule is a single line).
fn inline_script(rules: &[String]) -> String {
    if let [only] = rules
        && only.trim_start().starts_with("refactor")
    {
        return only.clone();
    }
    format!("{INLINE_PREFIX}{}\n}}\n", rules.join("\n"))
}

/// The wrapper `inline_script` puts before the first `-e` rule.
const INLINE_PREFIX: &str = "refactor \"inline\" { ";

#[derive(Parser, Debug)]
struct ListRulesArgs {
    /// Backend language id (e.g. "go").
    lang: String,
}

/// Parse CLI arguments and dispatch to the requested subcommand.
/// Returns the success exit code (0 or 10).
pub async fn run() -> Result<i32> {
    let argv: Vec<String> = std::env::args().collect();
    if let Some(value) = crate::help_json::requested(&Args::command(), &argv) {
        print_json(&value)?;
        return Ok(0);
    }
    let args = Args::parse();
    match args.command {
        Some(Commands::Refactor(refactor_args)) => run_refactor(refactor_args),
        Some(Commands::Server(server_args)) => {
            info!("Starting language server (port hint: {})", server_args.port);
            language_server::run(default_backends()).await;
            Ok(0)
        }
        Some(Commands::Mcp) => {
            info!("Starting MCP server on stdio");
            colab_mcp::run(default_backends()).map_err(|e| Error::Io {
                path: None,
                source: e,
            })?;
            Ok(0)
        }
        Some(Commands::Schema) => print_json(&discover::schema(&default_backends())),
        Some(Commands::Explain(args)) => {
            let (text, _) = args.script.load()?;
            let value = discover::explain_source(&text)?;
            print_json(&value)
        }
        Some(Commands::Find(args)) => run_find(args),
        Some(Commands::Rename(args)) => run_rename(args),
        Some(Commands::Mv(args)) => run_mv(args),
        Some(Commands::Ast(args)) => run_ast(args),
        Some(Commands::ListLanguages) => print_json(&discover::list_languages(&default_backends())),
        Some(Commands::ListRules(args)) => {
            let value = discover::list_rules(&default_backends(), &args.lang)?;
            print_json(&value)
        }
        Some(Commands::Pack(pack_args)) => match pack_args.cmd {
            PackCommand::List => print_json(&crate::packs::list()),
        },
        Some(Commands::Undo(args)) => run_undo(&args.from),
        None => Err(Error::Config(
            "no command provided; run with --help for usage".to_string(),
        )),
    }
}

fn print_json(value: &serde_json::Value) -> Result<i32> {
    let pretty = serde_json::to_string_pretty(value).expect("serialize JSON");
    let mut out = io::stdout().lock();
    writeln!(out, "{}", pretty).map_err(io_to_error)?;
    Ok(0)
}

fn io_to_error(err: io::Error) -> Error {
    Error::Io {
        path: None,
        source: err,
    }
}

/// Iterate `paths` directly (used by `--changed-since` and
/// `--staged`), invoking the visitor for files the transformer
/// considers relevant. Skips paths that no longer exist on disk
/// — `git diff` may report files that have been deleted.
fn run_against_paths<T, F>(transformer: &T, paths: &[PathBuf], visit: &mut F) -> Result<WalkOutcome>
where
    T: CodeTransformer + Sync,
    F: FnMut(FileChange) -> Result<()>,
{
    let mut outcome = WalkOutcome::default();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        outcome.files_visited += 1;
        if !transformer.is_file_relevant(path) {
            continue;
        }
        let before = match fs::read_to_string(path) {
            Ok(before) => before,
            // Match the tree walk: an unreadable file is recorded, not fatal.
            Err(e) => {
                outcome.skipped.push(colab_core::SkippedFile {
                    path: path.clone(),
                    reason: e.to_string(),
                });
                continue;
            }
        };
        let applied = transformer.apply_at(path, &before);
        visit(FileChange {
            path: path.clone(),
            before,
            after: applied.output,
            rules_fired: applied.rules_fired,
        })?;
    }
    Ok(outcome)
}

/// Drive a single transformer through the chosen file source
/// (`--changed-since` / `--staged` / tree walk). Pulled out so
/// the per-rule loop can reuse the same source-selection logic.
fn run_transformer<T, F>(
    transformer: &T,
    args: &RefactorArgs,
    targets: &[PathBuf],
    visit: &mut F,
) -> Result<WalkOutcome>
where
    T: CodeTransformer + Sync,
    F: FnMut(FileChange) -> Result<()>,
{
    if let Some(ref_) = args.changed_since.as_deref() {
        let files = git_changed_since(ref_)?;
        run_against_paths(transformer, &files, visit)
    } else if args.staged {
        let files = git_staged()?;
        run_against_paths(transformer, &files, visit)
    } else {
        let opts = WalkOptions {
            include: args.include.clone(),
            exclude: args.exclude.clone(),
            respect_gitignore: !args.no_ignore,
            follow_symlinks: false,
            jobs: resolve_jobs(args.jobs),
        };
        let mut outcome = WalkOutcome::default();
        for target in targets {
            let one = walker::walk_with(transformer, target, &opts, visit)?;
            outcome.files_visited += one.files_visited;
            outcome.skipped.extend(one.skipped);
        }
        Ok(outcome)
    }
}

fn git_changed_since(reference: &str) -> Result<Vec<PathBuf>> {
    git_paths(&["diff", "--name-only", "--diff-filter=ACMRT", reference])
}

fn git_staged() -> Result<Vec<PathBuf>> {
    git_paths(&["diff", "--name-only", "--cached", "--diff-filter=ACMRT"])
}

fn git_paths(args: &[&str]) -> Result<Vec<PathBuf>> {
    let output = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|e| Error::Config(format!("could not invoke git ({}): {}", args.join(" "), e)))?;
    if !output.status.success() {
        return Err(Error::Config(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// `--detail` value parser. Rejects unknown levels by name rather than
/// silently falling back, and lists the valid ones.
fn parse_detail(s: &str) -> std::result::Result<Detail, String> {
    Detail::parse(s).ok_or_else(|| {
        format!(
            "unknown detail level `{}`; expected one of: {}",
            s,
            Detail::NAMES.join(", ")
        )
    })
}

impl RefactorArgs {
    /// Resolve the rendering limits from the flags, applying the
    /// per-format default when `--detail` was not supplied.
    fn render_options(&self) -> RenderOptions {
        let detail = if self.summary_only {
            Detail::Summary
        } else {
            self.detail.unwrap_or_else(|| self.format.default_detail())
        };
        RenderOptions {
            detail,
            max_files: self.max_files,
            max_diff_bytes: self.max_diff_bytes,
        }
    }
}

/// Pick the worker-thread count from `--jobs`, falling back to
/// `COLAB_JOBS` if the flag wasn't set. `None` lets the walker
/// default to `num_cpus`.
fn resolve_jobs(flag: Option<usize>) -> Option<usize> {
    if let Some(n) = flag {
        return Some(n);
    }
    std::env::var("COLAB_JOBS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
}

fn run_refactor(args: RefactorArgs) -> Result<i32> {
    if args.stdin {
        return run_stdin(args);
    }

    change_working_dir(&args.change_dir)?;

    let backends = default_backends();
    let refactoring = args.script.compile(&backends)?;
    info!("Running script: {}", refactoring);

    let targets = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths.clone()
    };

    // `--verify` and `--commit-per-rule` need per-rule visibility
    // so failures can be attributed to (and reverted at) one rule.
    if args.verify.is_some() || args.commit_per_rule {
        return run_refactor_per_rule(&refactoring, &args, &targets);
    }

    // `--bisect` applies the full set, runs verify, and on failure
    // binary-searches the rule list to locate the breaking rule.
    if let Some(cmd) = args.bisect.clone() {
        return run_refactor_bisect(&refactoring, &args, &targets, &cmd);
    }

    let stdout_is_tty = io::stdout().is_terminal();
    let exec_mode = resolve_exec_mode(
        args.write,
        args.dry_run,
        args.check,
        args.format,
        stdout_is_tty,
    );
    let mut reporter = format::make_reporter(args.format, exec_mode, args.render_options());
    let started = Instant::now();
    let mut report = RunReport::with_rules(refactoring.rules.iter().map(|r| r.to_string()));

    let backup_dir = args.backup.clone();
    let mut visit = |change: FileChange| -> Result<()> {
        report.record(&change);
        if change.changed() && matches!(exec_mode, ExecMode::Write) {
            if let Some(dir) = &backup_dir {
                write_backup_snapshot(dir, &change.path, &change.before)?;
            }
            fs::write(&change.path, &change.after).map_err(|e| Error::io_at(&change.path, e))?;
        }
        reporter
            .report(&change)
            .map_err(|e| Error::io_at(&change.path, e))?;
        Ok(())
    };

    let outcome = run_transformer(&refactoring, &args, &targets, &mut visit)?;
    report.record_walk(outcome);
    colab_core::near_miss::probe_disk(&refactoring.rules, &mut report);
    report.elapsed = started.elapsed();
    reporter.finish(&report).map_err(io_to_error)?;

    Ok(
        if matches!(exec_mode, ExecMode::Check) && report.files_changed > 0 {
            10
        } else {
            0
        },
    )
}

/// Apply each rule individually, with optional `--verify`
/// auto-revert and `--commit-per-rule` git history.
///
/// Always writes (the verify / commit semantics require it). On
/// verify failure, restores the failed rule's per-file backups
/// and bubbles up an error so the CLI exit code reflects the
/// failure.
fn run_refactor_per_rule(
    refactoring: &codemod::Refactoring,
    args: &RefactorArgs,
    targets: &[PathBuf],
) -> Result<i32> {
    let mut reporter = format::make_reporter(args.format, ExecMode::Write, args.render_options());
    let started = Instant::now();
    let mut report = RunReport::with_rules(refactoring.rules.iter().map(|r| r.to_string()));
    // Each rule gets its own walk, so every file is visited once per
    // rule. Count the tree only once, from the first rule's pass.
    let mut files_visited = 0u64;

    for (idx, rule) in refactoring.rules.iter().enumerate() {
        let single = codemod::SingleRule::new(rule.as_ref());
        let rule_label = rule.to_string();
        info!(
            "[{}/{}] Applying rule: {}",
            idx + 1,
            refactoring.rules.len(),
            rule_label
        );

        // Per-rule backup: only the first time we touch a file's
        // pre-rule contents go in. If the same file is re-edited
        // by a later rule, *that* rule's backup captures the
        // post-this-rule state, which is what we want for an
        // independent revert.
        let mut backups: std::collections::HashMap<PathBuf, String> =
            std::collections::HashMap::new();
        let backup_dir = args.backup.clone();
        let mut visit = |mut change: FileChange| -> Result<()> {
            // `SingleRule` always reports index 0; re-index it against
            // the full script so the report reads the same as a normal run.
            change.rules_fired = change.rules_fired.into_iter().map(|_| idx).collect();
            report.record(&change);
            if change.changed() {
                backups
                    .entry(change.path.clone())
                    .or_insert_with(|| change.before.clone());
                if let Some(dir) = &backup_dir {
                    write_backup_snapshot(dir, &change.path, &change.before)?;
                }
                fs::write(&change.path, &change.after)
                    .map_err(|e| Error::io_at(&change.path, e))?;
            }
            reporter
                .report(&change)
                .map_err(|e| Error::io_at(&change.path, e))?;
            Ok(())
        };

        let outcome = run_transformer(&single, args, targets, &mut visit)?;
        // Each rule re-walks the same tree, so only the first pass's
        // visit count is meaningful; the rest would multiply it by the
        // rule count.
        if idx == 0 {
            files_visited = outcome.files_visited;
        }
        report.skipped.extend(outcome.skipped);

        if let Some(cmd) = &args.verify {
            info!("Verifying rule {} with `{}`", idx + 1, cmd);
            if !run_verify(cmd)? {
                // Restore this rule's backups and bail.
                for (path, before) in &backups {
                    fs::write(path, before).map_err(|e| Error::io_at(path, e))?;
                }
                return Err(Error::Config(format!(
                    "rule `{}` failed `--verify` (`{}`); reverted {} file(s)",
                    rule_label,
                    cmd,
                    backups.len()
                )));
            }
        }

        if args.commit_per_rule && !backups.is_empty() {
            commit_per_rule(&rule_label)?;
        }
    }

    report.files_visited = files_visited;
    report.elapsed = started.elapsed();
    reporter.finish(&report).map_err(io_to_error)?;
    Ok(0)
}

/// Run a `--verify` shell command. Returns `true` on success.
fn run_verify(cmd: &str) -> Result<bool> {
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .status()
        .map_err(|e| Error::Config(format!("could not spawn `{}`: {}", cmd, e)))?;
    Ok(status.success())
}

/// `git add -u && git commit -m "colab: <rule>"`. Bails if either
/// step fails. The "nothing to commit" case is treated as success
/// (a rule may have applied but produced no on-disk diff after
/// idempotent re-runs).
fn commit_per_rule(rule_label: &str) -> Result<()> {
    let add = std::process::Command::new("git")
        .args(["add", "-u"])
        .status()
        .map_err(|e| Error::Config(format!("git add failed to spawn: {}", e)))?;
    if !add.success() {
        return Err(Error::Config(format!(
            "git add -u failed (exit {}) for rule `{}`",
            add.code().unwrap_or(-1),
            rule_label
        )));
    }
    let message = format!("colab: {}", rule_label);
    let commit = std::process::Command::new("git")
        .args(["commit", "-m", &message])
        .output()
        .map_err(|e| Error::Config(format!("git commit failed to spawn: {}", e)))?;
    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr);
        // "nothing to commit" is treated as success — a rule may
        // have been a no-op even though the walker visited files.
        if stderr.contains("nothing to commit") {
            return Ok(());
        }
        return Err(Error::Config(format!(
            "git commit failed for rule `{}`: {}",
            rule_label,
            stderr.trim()
        )));
    }
    Ok(())
}

/// Apply the full refactoring, run `verify_cmd`, and on failure
/// binary-search the rule list to identify the single breaking
/// rule. Always restores the working tree to its pre-run state so
/// the user can inspect the failure cleanly.
fn run_refactor_bisect(
    refactoring: &codemod::Refactoring,
    args: &RefactorArgs,
    targets: &[PathBuf],
    verify_cmd: &str,
) -> Result<i32> {
    if refactoring.rules.is_empty() {
        info!("No rules to bisect");
        return Ok(0);
    }

    let mut reporter = format::make_reporter(args.format, ExecMode::Write, args.render_options());
    let started = Instant::now();
    let mut report = RunReport::with_rules(refactoring.rules.iter().map(|r| r.to_string()));

    // Phase 1: full apply, recording snapshots along the way.
    let mut snapshots: std::collections::HashMap<PathBuf, String> =
        std::collections::HashMap::new();
    let backup_dir = args.backup.clone();
    {
        let mut visit = |change: FileChange| -> Result<()> {
            report.record(&change);
            if change.changed() {
                snapshots
                    .entry(change.path.clone())
                    .or_insert_with(|| change.before.clone());
                if let Some(dir) = &backup_dir {
                    write_backup_snapshot(dir, &change.path, &change.before)?;
                }
                fs::write(&change.path, &change.after)
                    .map_err(|e| Error::io_at(&change.path, e))?;
            }
            reporter
                .report(&change)
                .map_err(|e| Error::io_at(&change.path, e))?;
            Ok(())
        };
        let outcome = run_transformer(refactoring, args, targets, &mut visit)?;
        report.record_walk(outcome);
    }

    // Phase 2: full-set verify.
    info!("Verifying with `{}`", verify_cmd);
    if run_verify(verify_cmd)? {
        report.elapsed = started.elapsed();
        reporter.finish(&report).map_err(io_to_error)?;
        return Ok(0);
    }

    // Phase 3: bisect.
    info!(
        "Verify failed; bisecting on {} rule(s)…",
        refactoring.rules.len()
    );
    let culprit_idx = bisect_rules(&refactoring.rules, &snapshots, args, targets, verify_cmd)?;
    let culprit_label = refactoring.rules[culprit_idx].to_string();

    // Phase 4: revert.
    for (path, before) in &snapshots {
        fs::write(path, before).map_err(|e| Error::io_at(path, e))?;
    }

    report.elapsed = started.elapsed();
    reporter.finish(&report).map_err(io_to_error)?;

    Err(Error::Config(format!(
        "rule {}/{} `{}` breaks `{}`; reverted {} file(s)",
        culprit_idx + 1,
        refactoring.rules.len(),
        culprit_label,
        verify_cmd,
        snapshots.len()
    )))
}

/// Binary search over `rules`. Invariant: `rules[..lo]` passes
/// verify, `rules[..hi]` fails. The single breaking rule is
/// `rules[lo]` once `hi - lo == 1`.
fn bisect_rules(
    rules: &[Box<dyn colab_core::Operation>],
    snapshots: &std::collections::HashMap<PathBuf, String>,
    args: &RefactorArgs,
    targets: &[PathBuf],
    verify_cmd: &str,
) -> Result<usize> {
    let mut lo = 0usize;
    let mut hi = rules.len();
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        // Restore the pre-run state for every snapshotted file.
        for (path, before) in snapshots {
            fs::write(path, before).map_err(|e| Error::io_at(path, e))?;
        }
        // Apply rules[..mid] sequentially.
        for rule in rules.iter().take(mid) {
            let single = codemod::SingleRule::new(rule.as_ref());
            run_transformer(
                &single,
                args,
                targets,
                &mut |change: FileChange| -> Result<()> {
                    if change.changed() {
                        fs::write(&change.path, &change.after)
                            .map_err(|e| Error::io_at(&change.path, e))?;
                    }
                    Ok(())
                },
            )?;
        }
        if run_verify(verify_cmd)? {
            info!("[bisect] rules[..{}] OK", mid);
            lo = mid;
        } else {
            info!("[bisect] rules[..{}] FAIL", mid);
            hi = mid;
        }
    }
    Ok(lo)
}

/// Save a single pre-modification snapshot to `<backup_dir>` with
/// path mirroring. Subsequent calls for the same file are no-ops
/// so the first (truly pre-rule) state is preserved across multi-
/// rule runs.
fn write_backup_snapshot(backup_dir: &Path, file: &Path, content: &str) -> Result<()> {
    let target = backup_target(backup_dir, file)?;
    if target.exists() {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
    }
    fs::write(&target, content).map_err(|e| Error::io_at(&target, e))?;
    Ok(())
}

/// Given a backup root and an original (possibly relative) path,
/// return the path inside the backup directory. Strips the leading
/// `/` on Unix so the structure mirrors the absolute filesystem
/// layout under the backup root.
fn backup_target(backup_dir: &Path, file: &Path) -> Result<PathBuf> {
    let canonical = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let rel = canonical
        .strip_prefix(std::path::MAIN_SEPARATOR.to_string())
        .unwrap_or(&canonical);
    Ok(backup_dir.join(rel))
}

/// Restore every file under `from` to the corresponding location
/// in the filesystem (mirroring the backup-target layout).
fn run_undo(from: &Path) -> Result<i32> {
    if !from.is_dir() {
        return Err(Error::Config(format!(
            "backup directory does not exist or is not a directory: {}",
            from.display()
        )));
    }
    // File operations first, so contents land back at original paths.
    let ops = crate::apply::undo_manifest(from)?;
    let mut count = 0u64;
    walk_undo(from, from, &mut count)?;
    if ops > 0 {
        info!("Undid {} file operation(s) through git", ops);
    }
    info!("Restored {} file(s) from {}", count, from.display());
    Ok(0)
}

fn walk_undo(root: &Path, current: &Path, count: &mut u64) -> Result<()> {
    let entries = fs::read_dir(current).map_err(|e| Error::io_at(current, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io_at(current, e))?;
        let path = entry.path();
        if path.is_dir() {
            walk_undo(root, &path, count)?;
        } else if current == root && entry.file_name() == crate::apply::MANIFEST {
            continue;
        } else if path.is_file() {
            let rel = path.strip_prefix(root).map_err(|_| {
                Error::Config(format!(
                    "path {} not under {}",
                    path.display(),
                    root.display()
                ))
            })?;
            let target = Path::new(&std::path::MAIN_SEPARATOR.to_string()).join(rel);
            let content = fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
            }
            fs::write(&target, content).map_err(|e| Error::io_at(&target, e))?;
            *count += 1;
        }
    }
    Ok(())
}

/// Resolve the effective exec mode given explicit flags and the
/// format-specific defaults.
fn resolve_exec_mode(
    write: bool,
    dry_run: bool,
    check: bool,
    format: Format,
    stdout_is_tty: bool,
) -> ExecMode {
    if check {
        ExecMode::Check
    } else if write {
        ExecMode::Write
    } else if dry_run {
        ExecMode::DryRun
    } else {
        format.default_exec_mode(stdout_is_tty)
    }
}

fn run_stdin(args: RefactorArgs) -> Result<i32> {
    let path = args
        .stdin_path
        .as_ref()
        .expect("clap requires --path with --stdin")
        .clone();

    if args.script.reads_stdin() {
        return Err(Error::Config(
            "`--script -` and `--stdin` both read stdin; pass the script as a file or with `-e`"
                .into(),
        ));
    }
    let backends = default_backends();
    let refactoring = args.script.compile(&backends)?;
    info!("Running script: {}", refactoring);

    let mut source = String::new();
    io::stdin()
        .read_to_string(&mut source)
        .map_err(io_to_error)?;

    let applied = refactoring.apply_at(&path, &source);

    let change = FileChange {
        path,
        before: source,
        after: applied.output,
        rules_fired: applied.rules_fired,
    };

    let mut report = RunReport::with_rules(refactoring.rules.iter().map(|r| r.to_string()));
    report.files_visited = 1;
    report.record(&change);
    colab_core::near_miss::probe(&refactoring.rules, &mut report, |_| {
        Some(change.before.clone())
    });

    match args.format {
        Format::Human => {
            // The transformed source *is* the output for `--stdin`; the
            // report would corrupt it, so it is deliberately omitted.
            let mut out = io::stdout().lock();
            out.write_all(change.after.as_bytes())
                .map_err(io_to_error)?;
        }
        Format::Json | Format::Ndjson | Format::Diff => {
            let mut reporter =
                format::make_reporter(args.format, ExecMode::DryRun, args.render_options());
            reporter
                .report(&change)
                .map_err(|e| Error::io_at(&change.path, e))?;
            reporter.finish(&report).map_err(io_to_error)?;
        }
    }

    Ok(if args.check && change.changed() {
        10
    } else {
        0
    })
}

fn run_find(args: FindArgs) -> Result<i32> {
    use colab_core::find::{FindDetail, FindReport, Selector};

    change_working_dir(&args.change_dir)?;
    let backends = default_backends();
    let refactoring = args.script.compile(&backends)?;
    let rules = &refactoring.rules;
    let detail = match args.detail {
        FindDetailArg::Concise => FindDetail::Concise,
        FindDetailArg::Detailed => FindDetail::Detailed,
    };
    let mut report = FindReport::new(rules, args.max_results, detail);

    let opts = WalkOptions {
        include: args.include.clone(),
        exclude: args.exclude.clone(),
        respect_gitignore: !args.no_ignore,
        follow_symlinks: false,
        jobs: None,
    };
    let targets = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths.clone()
    };
    let selector = Selector { rules };
    for target in &targets {
        let outcome = walker::walk_with(&selector, target, &opts, &mut |change| {
            report.record(rules, &change.path, &change.before);
            Ok(())
        })?;
        report.run.record_walk(outcome);
    }
    colab_core::near_miss::probe_disk(rules, &mut report.run);

    match args.format {
        FindFormat::Json => {
            let mut out = io::stdout().lock();
            writeln!(out, "{}", render::find_json(&report)).map_err(io_to_error)?;
        }
        FindFormat::Human => {
            let mut out = io::stdout().lock();
            render::write_find_human(&mut out, &report).map_err(io_to_error)?;
        }
    }
    Ok(0)
}

/// Markers that identify a project root for a language server.
const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "Cargo.toml",
    "go.mod",
    "package.json",
    "tsconfig.json",
    "pyproject.toml",
    "setup.py",
    "compile_commands.json",
    "CMakeLists.txt",
    "Package.swift",
    "Gemfile",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "composer.json",
];

fn project_root(file: &Path) -> Option<PathBuf> {
    file.ancestors()
        .skip(1)
        .find(|dir| PROJECT_MARKERS.iter().any(|m| dir.join(m).exists()))
        .map(Path::to_path_buf)
}

fn run_rename(args: RenameArgs) -> Result<i32> {
    use colab_lsp_client::{RenameRequest, ServerCommand};

    change_working_dir(&args.change_dir)?;
    let cwd = std::env::current_dir().map_err(io_to_error)?;

    // Where is the symbol?
    let (file, offset) = if let Some(at) = &args.at {
        let mut parts = at.rsplitn(3, ':');
        let (column, line, file) = (parts.next(), parts.next(), parts.next());
        let (Some(file), Some(line), Some(column)) = (file, line, column) else {
            return Err(Error::Config(format!(
                "--at wants FILE:LINE:COLUMN, got `{at}`"
            )));
        };
        let (Ok(line), Ok(column)) = (line.parse::<usize>(), column.parse::<usize>()) else {
            return Err(Error::Config(format!(
                "--at wants numeric LINE and COLUMN, got `{at}`"
            )));
        };
        let file = cwd.join(file);
        let text = fs::read_to_string(&file).map_err(|e| Error::io_at(&file, e))?;
        let offset = colab_lsp_client::offset_of_line_column(&text, line, column)
            .ok_or_else(|| Error::Config(format!("{} has no line {line}", file.display())))?;
        (file, offset)
    } else {
        let name = args.symbol.as_deref().expect("clap: --symbol or --at");
        let file = cwd.join(args.in_file.as_ref().expect("clap: --symbol requires --in"));
        let text = fs::read_to_string(&file).map_err(|e| Error::io_at(&file, e))?;
        let offset = colab_lsp_client::first_occurrence(&text, name).ok_or_else(|| {
            Error::Config(format!("`{name}` does not occur in {}", file.display()))
        })?;
        (file, offset)
    };
    let file = fs::canonicalize(&file).map_err(|e| Error::io_at(&file, e))?;

    let server = match &args.lsp {
        Some(command) => ServerCommand::parse(command)?,
        None => ServerCommand::default_for(&file).ok_or_else(|| {
            Error::Config(format!(
                "no default language server for {}; pass --lsp '<command>'",
                file.display()
            ))
        })?,
    };
    let root = match &args.root {
        Some(root) => fs::canonicalize(root).map_err(|e| Error::io_at(root, e))?,
        None => project_root(&file).unwrap_or_else(|| cwd.clone()),
    };

    // File operations (a class renamed with its file) only where git can
    // make and undo them; elsewhere the server is told not to send any.
    let file_operations = crate::git::Repo::discover(&root).is_some();
    info!("Renaming via `{}` in {}", server.display(), root.display());
    let started = Instant::now();
    let outcome = colab_lsp_client::rename(
        &server,
        &RenameRequest {
            root,
            file,
            offset,
            new_name: args.to.clone(),
            timeout: std::time::Duration::from_secs(args.timeout),
            settle: std::time::Duration::from_millis(500),
            file_operations,
        },
    )?;

    let label = format!(
        "lsp rename `{}` -> `{}` via {}",
        outcome.old_name.as_deref().unwrap_or("?"),
        args.to,
        server.program
    );
    run_change_set(
        &label,
        outcome.changes,
        Vec::new(),
        &args.apply,
        &cwd,
        started,
    )
}

/// `colab mv`: ask the server what the move needs, then make it with git.
fn run_mv(args: MvArgs) -> Result<i32> {
    use colab_core::fileops::{ChangeSet, FileOp};
    use colab_lsp_client::{MoveRequest, ServerCommand};

    change_working_dir(&args.change_dir)?;
    let cwd = std::env::current_dir().map_err(io_to_error)?;
    let from = fs::canonicalize(cwd.join(&args.from)).map_err(|e| Error::io_at(&args.from, e))?;
    let mut to = cwd.join(&args.to);
    if to.is_dir() {
        let name = from
            .file_name()
            .ok_or_else(|| Error::Config(format!("cannot move {}", from.display())))?;
        to = to.join(name);
    }
    // Resolve `to` through its parent, so it compares equal to the
    // canonical paths a server sends back.
    let to = match to.parent().and_then(|p| fs::canonicalize(p).ok()) {
        Some(parent) => parent.join(to.file_name().unwrap_or_default()),
        None => to,
    };
    if to.exists() {
        return Err(Error::Config(format!(
            "{} already exists; colab does not overwrite files",
            to.display()
        )));
    }
    let root = match &args.root {
        Some(root) => fs::canonicalize(root).map_err(|e| Error::io_at(root, e))?,
        None => project_root(&from).unwrap_or_else(|| cwd.clone()),
    };

    let server = if args.no_lsp {
        None
    } else {
        match &args.lsp {
            Some(command) => Some(ServerCommand::parse(command)?),
            None => ServerCommand::default_for(&from),
        }
    };
    let started = Instant::now();
    let mut warnings = Vec::new();
    let (changes, label) = match &server {
        Some(server) => {
            info!("Moving via `{}` in {}", server.display(), root.display());
            let outcome = colab_lsp_client::will_rename(
                server,
                &MoveRequest {
                    root,
                    from: from.clone(),
                    to: to.clone(),
                    timeout: std::time::Duration::from_secs(args.timeout),
                    settle: std::time::Duration::from_millis(500),
                },
            )?;
            if !outcome.references_updated {
                warnings.push(format!(
                    "`{}` does not update references when a file moves (no workspace.fileOperations.willRename); only the file was moved",
                    server.program
                ));
            }
            (outcome.changes, format!("move via {}", server.program))
        }
        None => {
            if !args.no_lsp {
                warnings.push(format!(
                    "no default language server for {}; only the file was moved (pass --lsp '<command>' to update references)",
                    from.display()
                ));
            }
            let set = ChangeSet {
                ops: vec![FileOp::Move { from, to }],
                edits: Vec::new(),
            };
            (set, "move via git".to_string())
        }
    };
    run_change_set(&label, changes, warnings, &args.apply, &cwd, started)
}

/// Report, and in write mode apply, a language server's answer: file
/// operations through git first, then edits — with `--check`,
/// `--backup`, and `--verify` behaving as for `refactor`.
fn run_change_set(
    label: &str,
    set: colab_core::fileops::ChangeSet,
    mut warnings: Vec<String>,
    flags: &ApplyFlags,
    cwd: &Path,
    started: Instant,
) -> Result<i32> {
    let exec_mode = if flags.verify.is_some() {
        ExecMode::Write
    } else {
        resolve_exec_mode(
            flags.write,
            flags.dry_run,
            flags.check,
            flags.format,
            io::stdout().is_terminal(),
        )
    };
    let options = crate::apply::ApplyOptions {
        allow_dirty: flags.allow_dirty,
        commit_moves: flags.commit_moves,
        backup: flags.backup.clone(),
    };
    let prepared = crate::apply::prepare(&set, &options)?;
    warnings.extend(prepared.warnings.iter().cloned());

    let render_options = RenderOptions {
        detail: flags
            .detail
            .unwrap_or_else(|| flags.format.default_detail()),
        max_files: render::DEFAULT_MAX_FILES,
        max_diff_bytes: render::DEFAULT_MAX_DIFF_BYTES,
    };
    let mut reporter = format::make_reporter(flags.format, exec_mode, render_options);
    let mut report = RunReport::with_rules([label]);
    // Report paths relative to where the user is, like `refactor`.
    let relative = |p: &Path| p.strip_prefix(cwd).unwrap_or(p).to_path_buf();
    report.files_visited = set.edits.len() as u64;
    report.file_ops = set.ops.iter().map(|op| op.map_paths(relative)).collect();
    report.warnings = warnings;
    for op in &report.file_ops {
        reporter.file_op(op).map_err(io_to_error)?;
    }

    let applied = if matches!(exec_mode, ExecMode::Write) {
        Some(crate::apply::apply(
            &set,
            &prepared,
            &options,
            |path, content| match &flags.backup {
                Some(dir) => write_backup_snapshot(dir, path, content),
                None => Ok(()),
            },
        )?)
    } else {
        None
    };
    for change in &set.edits {
        let mut change = change.clone();
        change.path = relative(&change.path);
        report.record(&change);
        reporter
            .report(&change)
            .map_err(|e| Error::io_at(&change.path, e))?;
    }
    if report.rules[0].files_matched == 0 && !set.ops.is_empty() {
        // A pure move did something although it edited no file.
        report.rules[0].files_matched = set.ops.len() as u64;
    }

    if let (Some(cmd), Some(applied)) = (&flags.verify, &applied) {
        info!("Verifying with `{cmd}`");
        if !run_verify(cmd)? {
            crate::apply::revert(&set, &prepared, applied)?;
            return Err(Error::Config(format!(
                "`{label}` failed `--verify` (`{cmd}`); undid {} file operation(s) and restored {} file(s)",
                applied.done.len(),
                set.edits.len()
            )));
        }
    }

    report.elapsed = started.elapsed();
    reporter.finish(&report).map_err(io_to_error)?;
    let pending = report.files_changed > 0 || !report.file_ops.is_empty();
    Ok(if matches!(exec_mode, ExecMode::Check) && pending {
        10
    } else {
        0
    })
}

fn run_ast(args: AstArgs) -> Result<i32> {
    let backends = default_backends();
    let value = match (&args.pattern, args.stdin) {
        (Some(pattern), _) => discover::ast_of_pattern(&backends, &args.lang, pattern)?,
        (None, true) => {
            let mut source = String::new();
            io::stdin()
                .read_to_string(&mut source)
                .map_err(io_to_error)?;
            discover::ast_of_source(&backends, &args.lang, &source)?
        }
        (None, false) => {
            return Err(Error::Config(
                "`colab ast` needs `--pattern '<code>'` or `--stdin`".into(),
            ));
        }
    };
    match args.format {
        FindFormat::Json => print_json(&value),
        FindFormat::Human => {
            let mut out = io::stdout().lock();
            for key in ["context", "prefilter"] {
                if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
                    writeln!(out, "{key}: {}", text.replace('\n', "⏎")).map_err(io_to_error)?;
                }
            }
            if value.get("has_error").and_then(|v| v.as_bool()) == Some(true) {
                writeln!(
                    out,
                    "warning: the snippet has syntax errors (see ERROR / MISSING nodes)"
                )
                .map_err(io_to_error)?;
            }
            let tree = value.get("tree").and_then(|v| v.as_str()).unwrap_or("");
            write!(out, "{tree}").map_err(io_to_error)?;
            Ok(0)
        }
    }
}

fn change_working_dir(path: &Path) -> Result<()> {
    let canonical = fs::canonicalize(path).map_err(|e| Error::io_at(path, e))?;
    if !canonical.is_dir() {
        return Err(Error::Config(format!(
            "--change-dir target is not a directory: {}",
            canonical.display()
        )));
    }
    std::env::set_current_dir(&canonical).map_err(|e| Error::io_at(&canonical, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_flag_overrides_format_default() {
        assert_eq!(
            resolve_exec_mode(false, false, true, Format::Human, true),
            ExecMode::Check
        );
    }

    #[test]
    fn write_flag_overrides_format_default() {
        assert_eq!(
            resolve_exec_mode(true, false, false, Format::Json, false),
            ExecMode::Write
        );
    }

    #[test]
    fn human_on_tty_defaults_to_write() {
        assert_eq!(
            resolve_exec_mode(false, false, false, Format::Human, true),
            ExecMode::Write
        );
    }

    #[test]
    fn human_on_non_tty_defaults_to_dry_run() {
        assert_eq!(
            resolve_exec_mode(false, false, false, Format::Human, false),
            ExecMode::DryRun
        );
    }

    #[test]
    fn json_defaults_to_dry_run_on_tty() {
        assert_eq!(
            resolve_exec_mode(false, false, false, Format::Json, true),
            ExecMode::DryRun
        );
    }

    #[test]
    fn diff_defaults_to_dry_run() {
        assert_eq!(
            resolve_exec_mode(false, false, false, Format::Diff, true),
            ExecMode::DryRun
        );
    }
}
