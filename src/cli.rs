//! Command-line interface for the colab binary.
//!
//! The CLI is split between argument parsing (driven by `clap`) and the
//! per-subcommand handlers below. Each handler returns
//! [`crate::error::Result`] so failures bubble up to `main` for unified
//! reporting.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use log::info;

use crate::codemod;
use crate::error::{Error, Result};
use crate::language_server;
use crate::walker;

static VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    ".",
    include_str!(concat!(env!("OUT_DIR"), "/version.txt"))
);

#[derive(Parser, Debug)]
#[command(
    color = clap::ColorChoice::Auto,
    author = "Graham Brooks",
    version = VERSION,
    about = "Scripted, AST-aware code refactoring",
    long_about = r#"
Code Lab (colab) is a command-line code refactoring (or 'codemod') tool.

It reads a small DSL describing a refactoring rule, parses target source
files with tree-sitter, and rewrites them in place. Use `colab refactor`
to run a script against a directory tree, or `colab server` to start the
LSP stub.
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

    /// Files or directories to process. Defaults to the (possibly
    /// changed) working directory if none are supplied.
    paths: Vec<PathBuf>,
}

/// Parse CLI arguments and dispatch to the requested subcommand.
pub async fn run() -> Result<()> {
    let args = Args::parse();
    match args.command {
        Some(Commands::Refactor(refactor_args)) => run_refactor(refactor_args),
        Some(Commands::Server(server_args)) => {
            info!("Starting language server (port hint: {})", server_args.port);
            language_server::run().await;
            Ok(())
        }
        None => Err(Error::Config(
            "no command provided; run with --help for usage".to_string(),
        )),
    }
}

fn run_refactor(args: RefactorArgs) -> Result<()> {
    change_working_dir(&args.change_dir)?;

    let script = fs::read_to_string(&args.script_path)
        .map_err(|e| Error::io_at(&args.script_path, e))?;
    let refactoring = codemod::compile(&script)?;
    info!("Running script: {}", refactoring);

    let targets = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths
    };

    for target in targets {
        walker::process_path(&refactoring, &target)?;
    }
    Ok(())
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
