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
    // The packs read inside the project's own expression macros too.
    if let Some(run) = colab_core::run::Run::current() {
        run.set_macros(config.macros.clone());
    }
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
        use_project_macros();
        codemod::compile_at_source(&text, &base, backends)
    }
}

/// The nearest `colab.toml`'s `[patterns] macros`, set on the current run
/// so patterns read inside the project's own expression macros. A
/// `colab.toml` that does not parse is `colab fix`'s to report; here it is
/// only a warning, so it never stops a script from running.
fn use_project_macros() {
    let Some(run) = colab_core::run::Run::current() else {
        return;
    };
    match crate::config::find() {
        Ok(Some(config)) => run.set_macros(config.macros),
        Ok(None) => {}
        Err(e) => log::warn!("ignoring colab.toml's [patterns]: {e}"),
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
