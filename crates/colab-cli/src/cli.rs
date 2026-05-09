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

use colab_core::walker::{self, FileChange};
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
--script foo.codemod` to inspect a parsed script as JSON, or `colab
server` to start the LSP stub.

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
    /// Print the JSON capability schema for every registered backend.
    Schema,
    /// Parse a script and print its IR as JSON without running it.
    Explain(ExplainArgs),
    /// List every language backend the binary knows about.
    ListLanguages,
    /// List the modules and actions a backend supports.
    ListRules(ListRulesArgs),
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
            language_server::run().await;
            Ok(0)
        }
        Some(Commands::Schema) => print_json(&discover::schema(&default_backends())),
        Some(Commands::Explain(args)) => {
            let value = discover::explain(&args.script_path)?;
            print_json(&value)
        }
        Some(Commands::ListLanguages) => {
            print_json(&discover::list_languages(&default_backends()))
        }
        Some(Commands::ListRules(args)) => {
            let value = discover::list_rules(&default_backends(), &args.lang)?;
            print_json(&value)
        }
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

fn run_refactor(args: RefactorArgs) -> Result<i32> {
    if args.stdin {
        return run_stdin(args);
    }

    change_working_dir(&args.change_dir)?;

    let script = fs::read_to_string(&args.script_path)
        .map_err(|e| Error::io_at(&args.script_path, e))?;
    let backends = default_backends();
    let refactoring = codemod::compile(&script, &backends)?;
    info!("Running script: {}", refactoring);

    let stdout_is_tty = io::stdout().is_terminal();
    let exec_mode = resolve_exec_mode(args.write, args.dry_run, args.check, args.format, stdout_is_tty);
    let mut reporter = format::make_reporter(args.format, exec_mode);

    let targets = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths.clone()
    };

    let mut would_change = 0u32;
    for target in &targets {
        walker::walk(&refactoring, target, &mut |change: FileChange| -> Result<()> {
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
        })?;
    }
    reporter.finish().map_err(io_to_error)?;

    Ok(if matches!(exec_mode, ExecMode::Check) && would_change > 0 {
        10
    } else {
        0
    })
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

    let script = fs::read_to_string(&args.script_path)
        .map_err(|e| Error::io_at(&args.script_path, e))?;
    let backends = default_backends();
    let refactoring = codemod::compile(&script, &backends)?;
    info!("Running script: {}", refactoring);

    let mut source = String::new();
    io::stdin().read_to_string(&mut source).map_err(io_to_error)?;

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
            out.write_all(change.after.as_bytes()).map_err(io_to_error)?;
        }
        Format::Json | Format::Ndjson | Format::Diff => {
            let mut reporter = format::make_reporter(args.format, ExecMode::DryRun);
            reporter
                .report(&change)
                .map_err(|e| Error::io_at(&change.path, e))?;
            reporter.finish().map_err(io_to_error)?;
        }
    }

    Ok(if args.check && change.changed() { 10 } else { 0 })
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
