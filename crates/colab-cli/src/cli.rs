//! Command-line interface for the colab binary.
//!
//! Parses arguments via `clap`, dispatches to per-subcommand
//! handlers, and returns the success exit code (0 or 10) to `main`.
//! Errors bubble up via [`colab_core::Error`] and `main` maps them to
//! the documented exit-code table.
use std::fmt::Write as _;

use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser};
use log::info;

use std::time::Instant;

use colab_core::render::{self, Detail, RenderOptions};
use colab_core::report::RunReport;
use colab_core::walker::{FileChange, WalkOptions};
use colab_core::{BackendRegistry, Error, Result};
use colab_dsl as codemod;
use colab_dsl::pipeline::{Source, semantic_failure};

use crate::discover;
use crate::format::{self, ExecMode, Format};
use crate::language_server;

mod mv_section;
use mv_section::*;
mod phased;
use phased::*;
mod targets;
use targets::*;
mod refactor;
use refactor::*;
mod split;
use split::*;
mod writes;
pub(crate) use writes::LinkCheck;
use writes::*;
mod bisect;
pub(crate) use bisect::*;
mod undo;
use undo::*;
mod stdin;
use stdin::*;
mod find;
use find::*;
mod change_set;
pub(crate) use change_set::*;

pub(crate) static VERSION: &str = concat!(
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
refactor` to run a script (or `-e` rules), `colab fix` to run the packs
a repository's colab.toml declares, and `colab find` / `colab ast` to
search and debug patterns. `colab rename` / `mv` / `action` /
`references` / `safe-delete` delegate to the language's own server;
`colab outline` / `section` / `links` read Markdown. `colab schema` /
`list-languages` / `list-rules <lang>` describe the capabilities,
`colab explain` prints a parsed script as JSON, `colab server` is the
LSP for .colab files, and `colab mcp` the Model Context Protocol
server.

Exit codes:
   0   Success (no changes needed, or --write succeeded).
   1   Generic / configuration error.
   2   Script parse error.
   3   Unsupported namespace or operation.
   4   I/O error.
  10   --check found changes that would be made.
  11   --write found conflicts and wrote nothing (see --allow-conflicts).
  12   colab test: a pack's case failed.
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
    /// Run the packs `colab.toml` declares, as one run, with its `[run]`
    /// settings: `colab fix --check` in CI, `colab fix` to apply.
    Fix(FixArgs),
    /// Start the colab language server over stdio.
    Server(ServerArgs),
    /// Start the colab MCP server (Model Context Protocol) over stdio.
    Mcp,
    /// Print the JSON capability schema for every registered backend.
    Schema,
    /// Search: report where each rule matches, without changing anything.
    Find(FindArgs),
    /// Review a large run by rule, from its `--format ndjson --detail diff`
    /// log: counts, most-edited files, and pages of each rule's diffs.
    Review(crate::review::ReviewArgs),
    /// Scope-aware rename, delegated to the language's own server
    /// (rust-analyzer, gopls, clangd, …) and reported like a codemod.
    Rename(RenameArgs),
    /// Move a file or directory with `git mv`, letting the language server
    /// update what refers to it.
    Mv(MvArgs),
    /// Run one of the language server's code actions — extract, inline,
    /// change signature, … — and report it like a codemod.
    Action(crate::lsp_commands::ActionArgs),
    /// List the references the language server finds to a symbol.
    References(crate::lsp_commands::ReferencesArgs),
    /// List who calls a function, or what it calls (`--outgoing`), as
    /// the language server's call hierarchy answers.
    Calls(crate::lsp_commands::CallsArgs),
    /// List what implements an interface, trait or abstract method, as
    /// the language server answers.
    Implementations(crate::lsp_commands::ImplementationsArgs),
    /// Delete a declaration only if nothing else refers to it.
    SafeDelete(crate::lsp_commands::SafeDeleteArgs),
    /// Print the syntax tree of a snippet or a pattern, to see why a
    /// pattern does or does not match.
    Ast(AstArgs),
    /// The heading outline of Markdown files: each section's address,
    /// anchor, line span, and the tables, lists and fences it holds.
    Outline(crate::markdown::OutlineArgs),
    /// Print one Markdown section, by heading path or anchor.
    Section(crate::markdown::SectionArgs),
    /// Move a Markdown section, subsections and all, to another file:
    /// re-levelled where it lands, links to it repointed, as one run.
    MvSection(MvSectionArgs),
    /// List broken links in Markdown files (missing files or anchors);
    /// exit 10 when there are any.
    Links(crate::markdown::LinksArgs),
    /// Keep a language server running between commands (started for you
    /// by default; `COLAB_KEEP_SERVER=0` turns that off); `--stop` stops
    /// every one.
    LspDaemon(LspDaemonArgs),
    /// Parse a script and print its IR as JSON without running it.
    Explain(ExplainArgs),
    /// List every language backend the binary knows about.
    ListLanguages,
    /// List the modules and actions a backend supports.
    ListRules(ListRulesArgs),
    /// Edit one declaration by name path (`Config/load`): replace it, or
    /// insert text before or after it, previewed like any run.
    Edit(crate::symbol_edit::EditArgs),
    /// Wire colab into Claude Code, Cursor or Codex (MCP server, skill,
    /// post-edit hook), or take it out again: `install` / `uninstall`.
    Agent(crate::agent::AgentArgs),
    /// Agent hooks: `colab hook post-edit` runs the repository's packs on
    /// the file an agent just wrote (exit 2 with findings on stderr).
    Hook(crate::hook::HookArgs),
    /// Manage discoverable codemod packs.
    Pack(PackArgs),
    /// Run one migration across many repositories (a `fleet.toml`): a
    /// canary first, a branch and a PR each with --write --pr.
    Fleet(crate::fleet::FleetArgs),
    /// Check a repository's packs against the cases beside them
    /// (`<pack>.tests/<case>/{input,expected}/`); exit 12 if one fails.
    Test(crate::pack_test::TestArgs),
    /// Restore files from a `--backup <dir>` produced by a prior
    /// `colab refactor`, `rename`, or `mv` run, undoing its file
    /// operations (through git) first.
    Undo(UndoArgs),
    /// Print colab's license and those of the crates the binary is built
    /// from, with their texts (THIRD-PARTY-LICENSES.txt).
    Licenses,
}

#[derive(Parser, Debug)]
struct LspDaemonArgs {
    /// Stop every kept server of this user, and exit.
    #[arg(long)]
    stop: bool,

    /// The socket to listen on.
    #[arg(long, value_name = "PATH", required_unless_present = "stop")]
    socket: Option<PathBuf>,

    /// The project root to start the server in (with `--stop`: stop only
    /// the servers for this root).
    #[arg(long, value_name = "DIR", required_unless_present = "stop")]
    root: Option<PathBuf>,

    /// Seconds without a client before the server is shut down.
    #[arg(long, value_name = "SECS", default_value_t = 600)]
    idle: u64,

    /// The server command, after `--`.
    #[arg(last = true)]
    server: Vec<String>,
}

#[cfg(not(unix))]
fn run_lsp_daemon(args: LspDaemonArgs) -> Result<u8> {
    if args.stop {
        println!("stopped 0 kept language server(s)");
        return Ok(0);
    }
    Err(Error::UnsupportedOperation(
        "kept language servers use Unix sockets; on this platform each command starts its own"
            .into(),
    ))
}

#[cfg(unix)]
fn run_lsp_daemon(args: LspDaemonArgs) -> Result<u8> {
    if args.stop {
        let root = args
            .root
            .as_deref()
            .map(|r| r.canonicalize().unwrap_or(r.to_path_buf()));
        let n = colab_lsp_client::daemon::stop_all(root.as_deref());
        println!("stopped {n} kept language server(s)");
        return Ok(0);
    }
    let (Some(socket), Some(root)) = (args.socket, args.root) else {
        return Err(Error::Config("--socket and --root are required".into()));
    };
    let Some((program, rest)) = args.server.split_first() else {
        return Err(Error::Config("give the server command after `--`".into()));
    };
    colab_lsp_client::daemon::serve(
        program,
        rest,
        &root,
        &socket,
        std::time::Duration::from_secs(args.idle),
    )
    .map_err(|e| Error::io_at(&socket, e))?;
    Ok(0)
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
    /// List the packs built into colab (`colab.toml`: `pack = "<name>"`)
    /// and those in `<repo>/.colab/packs/` and `~/.colab/packs/`.
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
    select: SelectArgs,

    /// The language server that answers `type` / `resolves` constraints,
    /// instead of each language's usual one (`gopls`, `rust-analyzer`, …).
    #[arg(long = "lsp", value_name = "COMMAND")]
    lsp: Option<String>,

    /// Seconds to wait for that server to start and index.
    #[arg(long = "lsp-timeout", value_name = "SECS", default_value_t = 120)]
    lsp_timeout: u64,

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

    /// Report only findings that are new since this git ref: the script
    /// also runs over the tree as it was there, and what it would change
    /// there too is left out. Never writes.
    #[arg(long, value_name = "REF", conflicts_with = "write")]
    baseline: Option<String>,

    /// Worker thread count. Defaults to the available parallelism. Falls back to
    /// the `COLAB_JOBS` env var when unset. Set to 1 to force
    /// sequential processing.
    #[arg(long, value_name = "N")]
    jobs: Option<usize>,

    /// Run this shell command after each rule's edits. A non-zero
    /// exit reverts that rule's changes and aborts the run.
    /// Implies `--write`. Every rule must pass on its own, so for rules
    /// that only work together (a new parameter and its callers) use
    /// `--bisect`, which checks once at the end.
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
    /// contents under `<dir>`, mirroring the file's absolute path. Pair
    /// with `colab undo
    /// --from <dir>` to roll back the run.
    #[arg(long, value_name = "DIR", conflicts_with_all = ["dry_run", "check"])]
    backup: Option<PathBuf>,

    /// Before writing, open each changed file in its language server,
    /// change it in memory, and treat every error diagnostic the change
    /// adds as a conflict. Cheaper than `--verify`'s full build, and it
    /// runs before anything is written. The server is chosen by extension
    /// (rust-analyzer, gopls, …); `--verify-lsp='<command>'` names one.
    #[arg(long = "verify-lsp", value_name = "CMD", num_args = 0..=1,
          require_equals = true, default_missing_value = "",
          conflicts_with_all = ["verify", "commit_per_rule"])]
    verify_lsp: Option<String>,

    /// Write even when a rule reports a conflict: a `symbol` rename to a
    /// name the file already uses, or a pattern rewrite that introduces a
    /// qualifier (`fmt.`) nothing imports. Without it, a run that finds
    /// any writes nothing and exits 11; every format lists them.
    #[arg(long = "allow-conflicts")]
    allow_conflicts: bool,

    /// After the rules, format each changed file before anything is
    /// written: with the command colab.toml's `[formatters]` gives its
    /// extension, or else its language server (organize imports, then
    /// format). Reported as its own rule (`tidy via <server or command>`)
    /// after the script's. `--tidy='<command>'` names one server for
    /// every file.
    #[arg(long, value_name = "CMD", num_args = 0..=1, require_equals = true,
          default_missing_value = "",
          conflicts_with_all = ["verify", "commit_per_rule"])]
    tidy: Option<String>,

    /// Make one git branch per rule instead of writing: each is `HEAD`
    /// plus that rule's changes alone, named `<PREFIX>/<NN>-<rule>`
    /// (default prefix `colab/<script>`), ready for one pull request per
    /// rule. The work tree, the index and the current branch are left
    /// alone; the files the rules change must have no uncommitted changes.
    #[arg(long = "split-pr", value_name = "PREFIX", num_args = 0..=1,
          require_equals = true, default_missing_value = "",
          conflicts_with_all = ["write", "dry_run", "check", "verify", "commit_per_rule",
                                "bisect", "tidy", "backup", "stdin"])]
    split_pr: Option<String>,

    /// With `--split-pr`: stack the branches. Each rule applies to the
    /// previous rule's output and its branch starts from the previous
    /// branch, for a chain of pull requests merged in order. A rule that
    /// only rewrites what an earlier one wrote gets a branch too.
    #[arg(long, requires = "split_pr")]
    stacked: bool,

    /// Skip the link check. Without it, a run that changes Markdown files
    /// checks every Markdown file under the paths, and each link the run
    /// would break (a renamed heading, a moved file) is a conflict.
    #[arg(long = "no-link-check")]
    no_link_check: bool,

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

/// `colab mv-section`.
#[derive(Parser, Debug)]
struct MvSectionArgs {
    /// The Markdown file the section is in.
    file: PathBuf,

    /// The section: a heading path (`"Plan > Old item"`) or `#anchor`.
    section: String,

    /// The file it moves to.
    #[arg(long, value_name = "FILE")]
    to: PathBuf,

    /// Make it the last child of this section of the target.
    #[arg(long, value_name = "SECTION", conflicts_with = "before")]
    under: Option<String>,

    /// Put it just before this section of the target, at its level.
    /// Without `--under` or `--before` it goes at the end, top level.
    #[arg(long, value_name = "SECTION")]
    before: Option<String>,

    /// Print the generated script and stop.
    #[arg(long = "show-script")]
    show_script: bool,

    /// Apply the move. Default for `--format human` on a TTY.
    #[arg(long, conflicts_with_all = ["dry_run", "check"])]
    write: bool,

    /// Report what would change without writing.
    #[arg(long = "dry-run", conflicts_with = "check")]
    dry_run: bool,

    /// Exit 10 if the move would change anything; write nothing.
    #[arg(long)]
    check: bool,

    /// Output format, as for `refactor`.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,

    /// Write even when the move would break a link elsewhere.
    #[arg(long = "allow-conflicts")]
    allow_conflicts: bool,

    /// Change to this directory first.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    change_dir: PathBuf,
}

/// `colab fix`: the packs `colab.toml` declares.
#[derive(Parser, Debug)]
struct FixArgs {
    #[command(flatten)]
    select: SelectArgs,

    /// Exit 10 if any pack would change a file (at a severity above
    /// `info`); write nothing. For CI.
    #[arg(long, conflicts_with_all = ["write", "dry_run"])]
    check: bool,

    /// Apply the changes. Default for `--format human` on a TTY. With
    /// `[run] check`, the command runs afterwards, and a failure is
    /// bisected to one rule and reverted, as with `refactor --bisect`.
    #[arg(long, conflicts_with = "dry_run")]
    write: bool,

    /// Report what would change without writing.
    #[arg(long = "dry-run")]
    dry_run: bool,

    /// Output format, as for `refactor`.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,

    /// `summary`, `counts` or `diff`, as for `refactor`.
    #[arg(long, value_name = "LEVEL")]
    detail: Option<String>,

    /// Start looking for `colab.toml` here instead of the working
    /// directory.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    change_dir: PathBuf,

    /// The config file to use, instead of the nearest `colab.toml` at or
    /// above the working directory (stopping at the repository root).
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Write even when a rule reports a conflict (see `refactor`).
    #[arg(long = "allow-conflicts")]
    allow_conflicts: bool,

    /// Report only findings new since this git ref (see `refactor`).
    #[arg(long, value_name = "REF", conflicts_with = "write")]
    baseline: Option<String>,

    /// Skip the Markdown link check (see `refactor`).
    #[arg(long = "no-link-check")]
    no_link_check: bool,
}

#[derive(Parser, Debug)]
struct FindArgs {
    #[command(flatten)]
    select: SelectArgs,

    /// The language server that answers `type` / `resolves` constraints,
    /// instead of each language's usual one (`gopls`, `rust-analyzer`, …).
    #[arg(long = "lsp", value_name = "COMMAND")]
    lsp: Option<String>,

    /// Seconds to wait for that server to start and index.
    #[arg(long = "lsp-timeout", value_name = "SECS", default_value_t = 120)]
    lsp_timeout: u64,

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

    /// List at most this many matches; the rest are counted. Default 50,
    /// or every match with `--table`.
    #[arg(long = "max-results", value_name = "N")]
    max_results: Option<usize>,

    /// Every match as a row: `path,line,column,rule,id`, a column per
    /// metavariable (`$X`), and the match `text`. `csv` or `ndjson`.
    #[arg(long, value_enum, value_name = "FORMAT")]
    table: Option<crate::table::TableFormat>,

    /// With `--table`: one row per distinct value of these columns
    /// (`$X`, `rule`, `id`, `path`), with a `count`, most common first.
    /// Repeatable.
    #[arg(long = "group-by", value_name = "COLUMN", requires = "table")]
    group_by: Vec<String>,

    /// Whitelist files via gitignore-syntax glob. Repeatable.
    #[arg(long = "include", value_name = "GLOB")]
    include: Vec<String>,

    /// Blacklist files via gitignore-syntax glob. Repeatable.
    #[arg(long = "exclude", value_name = "GLOB")]
    exclude: Vec<String>,

    /// Don't honour `.gitignore` or hidden-file rules.
    #[arg(long = "no-ignore")]
    no_ignore: bool,

    /// Worker thread count. Defaults to the available parallelism. Falls back to
    /// the `COLAB_JOBS` env var when unset. Set to 1 to search one file at a
    /// time. The results are the same, in the same order, for any count.
    #[arg(long, value_name = "N")]
    jobs: Option<usize>,

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

    /// Also change the old name where the server does not: whole-word
    /// matches in comments and string literals of source files, and in
    /// config and docs files (TOML, JSON, YAML, Markdown, text) under the
    /// root. Reported as its own rule. Without it they are only counted.
    #[arg(long = "include-text")]
    include_text: bool,

    #[command(flatten)]
    apply: ApplyFlags,
}

/// Output and write flags shared by the commands that apply a language
/// server's answer (`rename`, `mv`).
#[derive(clap::Args, Debug)]
pub(crate) struct ApplyFlags {
    /// Output format, as for `refactor`.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub(crate) format: Format,

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

    /// After the change, have the language server organize imports and
    /// format each changed file. Reported as its own rule (`tidy via
    /// <server>`), so you can see what the refactoring did and what
    /// tidying did.
    #[arg(long)]
    pub(crate) tidy: bool,

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

/// Each pack `config` declares, compiled (a git pack fetched and its own
/// cases passed first), in order: what `colab fix` and the post-edit hook
/// run.
pub(crate) fn compile_packs(
    config: &crate::config::Config,
    backends: &BackendRegistry,
) -> Result<Vec<codemod::Refactoring>> {
    let mut parts = Vec::new();
    for pack in &config.packs {
        parts.push(match pack {
            crate::config::PackRef::Builtin(name) => {
                let builtin = crate::packs::builtin(name).map_err(|e| {
                    Error::Config(format!(
                        "{}: {e}",
                        config.dir.join(crate::config::FILE).display()
                    ))
                })?;
                codemod::compile_at_source(
                    builtin.script,
                    &PathBuf::from(format!("<builtin {}>", builtin.name)),
                    backends,
                )?
            }
            crate::config::PackRef::Path(file) => {
                let file = config.dir.join(file);
                colab_core::script_file::warn_if_legacy(&file);
                let text = fs::read_to_string(&file).map_err(|e| Error::io_at(&file, e))?;
                codemod::compile_at_source(&text, &file, backends)?
            }
            crate::config::PackRef::Git { url, pin, path } => {
                let file = crate::git_packs::resolve(url, pin, path, backends)?;
                info!("Pack {} from {url} at {pin}", path.display());
                let text = fs::read_to_string(&file).map_err(|e| Error::io_at(&file, e))?;
                codemod::compile_at_source(&text, &file, backends)?
            }
        });
    }
    Ok(parts)
}

/// Which rules run, by `id`, id glob (`no-*`) or number (from 1).
#[derive(clap::Args, Debug, Default)]
struct SelectArgs {
    /// Run only the rules this selects: a rule's `id`, a glob over ids
    /// (`no-*`), or its number in the script. Repeatable.
    #[arg(long = "only", value_name = "SELECTOR")]
    only: Vec<String>,

    /// Leave out the rules this selects. Repeatable.
    #[arg(long = "off", value_name = "SELECTOR")]
    off: Vec<String>,

    /// Override the severity of the rules a selector selects:
    /// `--severity no-dbg=warning`. Repeatable; the last that applies wins.
    #[arg(long = "severity", value_name = "SELECTOR=LEVEL", value_parser = parse_severity)]
    severity: Vec<(String, colab_core::Severity)>,
}

fn parse_severity(text: &str) -> std::result::Result<(String, colab_core::Severity), String> {
    let (selector, level) = text
        .rsplit_once('=')
        .ok_or_else(|| format!("`{text}`: write `<selector>=<level>`, e.g. `no-dbg=warning`"))?;
    let level = colab_core::Severity::parse(level)
        .ok_or_else(|| format!("`{level}`: a severity is `error`, `warning` or `info`"))?;
    Ok((selector.to_string(), level))
}

impl SelectArgs {
    fn apply(&self, refactoring: &mut codemod::Refactoring) -> Result<()> {
        refactoring.select(&codemod::Selection {
            only: self.only.clone(),
            off: self.off.clone(),
            severity: self.severity.clone(),
        })
    }
}

/// Where the codemod script comes from: a file, stdin, or inline rules.
///
/// Exactly one of `--script` / `--rule` is required. Inline rules exist
/// so an agent (or a one-off shell command) can run a rule without
/// first writing a `.colab` file.
#[derive(clap::Args, Debug)]
#[group(required = true, multiple = false)]
struct ScriptArgs {
    /// Path to the codemod script. `-` reads the script from stdin;
    /// `builtin:<pack>` runs a pack built into colab (`colab pack list`).
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
            // `builtin:go/1.18-modern`: a pack compiled into colab, as
            // `colab pack list` names it. Its includes, if it had any,
            // would resolve against the working directory.
            Some(path) if path.to_string_lossy().starts_with("builtin:") => {
                let name = path.to_string_lossy();
                let pack = crate::packs::builtin(&name["builtin:".len()..])?;
                Ok((
                    pack.script.to_string(),
                    cwd.join(format!("<builtin {}>", pack.name)),
                ))
            }
            Some(path) => {
                colab_core::script_file::warn_if_legacy(path);
                let text = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
                Ok((text, path.clone()))
            }
            None => Ok((inline_script(&self.rules), cwd.join("<inline>"))),
        }
    }

    fn compile(&self, backends: &BackendRegistry) -> Result<codemod::Refactoring> {
        let (text, base) = self.load()?;
        codemod::compile_at_source(&text, &base, backends)
    }
}

/// The `-e` rules as one script, one per line: bare rules, which the
/// grammar takes as they are, so a parse error's line is the rule's
/// position in the `-e` list (when each rule is one line). A single `-e`
/// may be a whole `refactor "…" { … }` script.
fn inline_script(rules: &[String]) -> String {
    rules.join("\n")
}

#[derive(Parser, Debug)]
struct ListRulesArgs {
    /// Backend language id (e.g. "go").
    lang: String,
}

/// Parse CLI arguments and dispatch to the requested subcommand.
/// Returns the success exit code (0 or 10).
///
/// The command is one [`colab_core::run::Run`]: its oracle, scan facts
/// and unanswerable questions belong to it alone.
///
/// With an OTLP endpoint configured (environment or user config file), the command is also a trace
/// (`crate::telemetry`): its root span ends, and is exported, here.
pub fn run() -> Result<u8> {
    let argv: Vec<String> = std::env::args().collect();
    crate::telemetry::init(crate::telemetry::long_running(&argv));
    let subcommands: Vec<String> = Args::command()
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    let mut span = crate::telemetry::command_span(&argv, &subcommands);
    let outcome = colab_core::run::Run::new().enter(dispatch);
    crate::telemetry::finish(&mut span, &outcome);
    outcome
}

fn dispatch() -> Result<u8> {
    let argv: Vec<String> = std::env::args().collect();
    if let Some(value) = crate::help_json::requested(&Args::command(), &argv) {
        print_json(&value)?;
        return Ok(0);
    }
    let args = Args::parse();
    match args.command {
        Some(Commands::Refactor(refactor_args)) => run_refactor(refactor_args),
        Some(Commands::Fix(fix_args)) => run_fix(fix_args),
        Some(Commands::MvSection(args)) => run_mv_section(args),
        Some(Commands::Review(review_args)) => crate::review::run(review_args),
        Some(Commands::Test(args)) => crate::pack_test::run(args, &default_backends()),
        Some(Commands::Fleet(args)) => crate::fleet::run(args),
        Some(Commands::Hook(args)) => crate::hook::run(args, &default_backends()),
        Some(Commands::Agent(args)) => crate::agent::run(args),
        Some(Commands::Edit(args)) => crate::symbol_edit::run(args, &default_backends()),
        Some(Commands::Server(server_args)) => {
            info!("Starting language server (port hint: {})", server_args.port);
            // The only async command. Multi-threaded: the server runs
            // walks under `block_in_place`, which a current-thread
            // runtime does not allow.
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(io_to_error)?
                .block_on(language_server::run(default_backends()));
            Ok(0)
        }
        Some(Commands::Mcp) => {
            info!("Starting MCP server on stdio");
            colab_mcp::run_with(default_backends(), &crate::markdown::BinaryTools).map_err(
                |e| Error::Io {
                    path: None,
                    source: e,
                },
            )?;
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
        Some(Commands::Action(args)) => crate::lsp_commands::run_action(args),
        Some(Commands::References(args)) => crate::lsp_commands::run_references(args),
        Some(Commands::Calls(args)) => crate::lsp_commands::run_calls(args),
        Some(Commands::Implementations(args)) => crate::lsp_commands::run_implementations(args),
        Some(Commands::SafeDelete(args)) => crate::lsp_commands::run_safe_delete(args),
        Some(Commands::Ast(args)) => run_ast(args),
        Some(Commands::Outline(args)) => crate::markdown::run_outline(args),
        Some(Commands::Section(args)) => crate::markdown::run_section(args),
        Some(Commands::Links(args)) => crate::markdown::run_links(args),
        Some(Commands::LspDaemon(args)) => run_lsp_daemon(args),
        Some(Commands::ListLanguages) => print_json(&discover::list_languages(&default_backends())),
        Some(Commands::ListRules(args)) => {
            let value = discover::list_rules(&default_backends(), &args.lang)?;
            print_json(&value)
        }
        Some(Commands::Pack(pack_args)) => match pack_args.cmd {
            PackCommand::List => print_json(&crate::packs::list()),
        },
        Some(Commands::Undo(args)) => run_undo(&args.from),
        Some(Commands::Licenses) => print_licenses(),
        None => Err(Error::Config(
            "no command provided; run with --help for usage".to_string(),
        )),
    }
}

/// colab's own license, then every third-party one the binary carries:
/// a release archive holds only the binary, so their notices travel in it.
fn print_licenses() -> Result<u8> {
    const OWN: &str = include_str!("../../../LICENSE");
    const THIRD_PARTY: &str = include_str!("../../../THIRD-PARTY-LICENSES.txt");
    let mut out = io::stdout().lock();
    let written = writeln!(out, "colab\n\n{}\n\n{THIRD_PARTY}", OWN.trim_end());
    match written {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(0),
        other => other.map(|()| 0).map_err(io_to_error),
    }
}

fn print_json(value: &serde_json::Value) -> Result<u8> {
    let pretty = serde_json::to_string_pretty(value).expect("serialize JSON");
    let mut out = io::stdout().lock();
    writeln!(out, "{pretty}").map_err(io_to_error)?;
    Ok(0)
}

/// An I/O error with no file to name: stdout, stdin, a child process,
/// the working directory. Anything that touches a known path uses
/// [`Error::io_at`] instead; `colab_core::Error` has no `From<io::Error>`,
/// so a bare `?` cannot drop the path by accident.
pub(crate) fn io_to_error(err: io::Error) -> Error {
    Error::Io {
        path: None,
        source: err,
    }
}

pub(crate) fn change_working_dir(path: &Path) -> Result<()> {
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
