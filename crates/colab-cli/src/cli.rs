//! Command-line interface for the colab binary.
//!
//! Parses arguments via `clap`, dispatches to per-subcommand
//! handlers, and returns the success exit code (0 or 10) to `main`.
//! Errors bubble up via [`colab_core::Error`] and `main` maps them to
//! the documented exit-code table.

use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use clap::Parser;
use log::info;

use colab_core::walker::{self, FileChange, WalkOptions};
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

/// Build the registry of language backends compiled into this binary.
///
/// New `colab-lang-*` crates plug in here.
fn default_backends() -> BackendRegistry {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(colab_lang_go::GoBackend));
    registry.register(Box::new(colab_lang_java::JavaBackend));
    registry.register(Box::new(colab_lang_js::JsBackend));
    registry.register(Box::new(colab_lang_python::PythonBackend));
    registry.register(Box::new(colab_lang_rust::RustBackend));
    registry
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
}

#[derive(Parser, Debug)]
enum Commands {
    /// Run a codemod script against one or more paths.
    Refactor(RefactorArgs),
    /// Start the colab language server over stdio.
    Server(ServerArgs),
    /// Start the colab MCP server (Model Context Protocol) over stdio.
    Mcp,
    /// Print the JSON capability schema for every registered backend.
    Schema,
    /// Parse a script and print its IR as JSON without running it.
    Explain(ExplainArgs),
    /// List every language backend the binary knows about.
    ListLanguages,
    /// List the modules and actions a backend supports.
    ListRules(ListRulesArgs),
    /// Manage discoverable codemod packs.
    Pack(PackArgs),
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
    /// Path to the codemod script to execute.
    #[arg(long = "script", required = true)]
    script_path: PathBuf,

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

    /// Files or directories to process. Defaults to the (possibly
    /// changed) working directory if none are supplied. Ignored with
    /// `--stdin`.
    paths: Vec<PathBuf>,
}

#[derive(Parser, Debug)]
struct ExplainArgs {
    /// Path to the codemod script to parse.
    #[arg(long = "script", required = true)]
    script_path: PathBuf,
}

#[derive(Parser, Debug)]
struct ListRulesArgs {
    /// Backend language id (e.g. "go").
    lang: String,
}

/// Parse CLI arguments and dispatch to the requested subcommand.
/// Returns the success exit code (0 or 10).
pub async fn run() -> Result<i32> {
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
            let value = discover::explain(&args.script_path)?;
            print_json(&value)
        }
        Some(Commands::ListLanguages) => print_json(&discover::list_languages(&default_backends())),
        Some(Commands::ListRules(args)) => {
            let value = discover::list_rules(&default_backends(), &args.lang)?;
            print_json(&value)
        }
        Some(Commands::Pack(pack_args)) => match pack_args.cmd {
            PackCommand::List => print_json(&crate::packs::list()),
        },
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
/// `--staged`), invoking the visitor for files the refactoring
/// considers relevant. Skips paths that no longer exist on disk
/// — `git diff` may report files that have been deleted.
fn run_against_paths<F>(
    refactoring: &codemod::Refactoring,
    paths: &[PathBuf],
    visit: &mut F,
) -> Result<()>
where
    F: FnMut(FileChange) -> Result<()>,
{
    for path in paths {
        if !path.is_file() {
            continue;
        }
        if !refactoring.is_file_relevant(path) {
            continue;
        }
        let before = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
        let after = refactoring.apply(&before);
        visit(FileChange {
            path: path.clone(),
            before,
            after,
        })?;
    }
    Ok(())
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
        .map_err(|e| {
            Error::Config(format!(
                "could not invoke git ({}): {}",
                args.join(" "),
                e
            ))
        })?;
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
    let refactoring = codemod::compile_at_path(&args.script_path, &backends)?;
    info!("Running script: {}", refactoring);

    let stdout_is_tty = io::stdout().is_terminal();
    let exec_mode = resolve_exec_mode(
        args.write,
        args.dry_run,
        args.check,
        args.format,
        stdout_is_tty,
    );
    let mut reporter = format::make_reporter(args.format, exec_mode);

    let mut would_change = 0u32;
    let mut visit = |change: FileChange| -> Result<()> {
        if change.changed() {
            would_change += 1;
            if matches!(exec_mode, ExecMode::Write) {
                fs::write(&change.path, &change.after)
                    .map_err(|e| Error::io_at(&change.path, e))?;
            }
        }
        reporter
            .report(&change)
            .map_err(|e| Error::io_at(&change.path, e))?;
        Ok(())
    };

    if let Some(ref_) = args.changed_since.as_deref() {
        let files = git_changed_since(ref_)?;
        run_against_paths(&refactoring, &files, &mut visit)?;
    } else if args.staged {
        let files = git_staged()?;
        run_against_paths(&refactoring, &files, &mut visit)?;
    } else {
        let opts = WalkOptions {
            include: args.include.clone(),
            exclude: args.exclude.clone(),
            respect_gitignore: !args.no_ignore,
            follow_symlinks: false,
            jobs: resolve_jobs(args.jobs),
        };
        let targets = if args.paths.is_empty() {
            vec![PathBuf::from(".")]
        } else {
            args.paths.clone()
        };
        for target in &targets {
            walker::walk_with(&refactoring, target, &opts, &mut visit)?;
        }
    }
    reporter.finish().map_err(io_to_error)?;

    Ok(
        if matches!(exec_mode, ExecMode::Check) && would_change > 0 {
            10
        } else {
            0
        },
    )
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

    let script =
        fs::read_to_string(&args.script_path).map_err(|e| Error::io_at(&args.script_path, e))?;
    let backends = default_backends();
    let refactoring = codemod::compile(&script, &backends)?;
    info!("Running script: {}", refactoring);

    let mut source = String::new();
    io::stdin()
        .read_to_string(&mut source)
        .map_err(io_to_error)?;

    let after = if refactoring.is_file_relevant(&path) {
        refactoring.apply(&source)
    } else {
        source.clone()
    };

    let change = FileChange {
        path,
        before: source,
        after,
    };

    match args.format {
        Format::Human => {
            let mut out = io::stdout().lock();
            out.write_all(change.after.as_bytes())
                .map_err(io_to_error)?;
        }
        Format::Json | Format::Ndjson | Format::Diff => {
            let mut reporter = format::make_reporter(args.format, ExecMode::DryRun);
            reporter
                .report(&change)
                .map_err(|e| Error::io_at(&change.path, e))?;
            reporter.finish().map_err(io_to_error)?;
        }
    }

    Ok(if args.check && change.changed() {
        10
    } else {
        0
    })
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
