use crate::{codemod, language_server, refactor};
use clap::Parser;
use std::fs;
use std::path::Path;

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
about = "AST generator based on tree-sitter",
long_about = r#"
Code Lab (colab) command line code refactoring or 'codemod'.

Scripted refactoring at scale.

"#
)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

pub(crate) struct Cli {}

#[derive(Parser, Debug)]
enum Commands {
    Server(ServerArgs),
    Refactor(RefactorArgs),
}

#[derive(Parser, Debug)]
struct ServerArgs {
    #[arg(long, help = "Port to run the server on", default_value = "8080")]
    port: u16,
}

#[derive(Parser, Debug)]
struct RefactorArgs {
    #[arg(
        long = "script",
        help = "codemod Script to run against the codebase",
        required = true
    )]
    script_path: Option<String>,
    #[arg(
        short = 'C',
        long = "change-dir",
        help = "Change working directoryPaths to the files or directories to process",
        default_value = "."
    )]
    path: Option<String>,
    paths: Vec<String>,
}

impl Cli {
    pub(crate) fn new() -> Self {
        Cli {}
    }
    pub(crate) async fn run(&self) {
        let args = Args::parse();

        match args.command {
            Some(Commands::Refactor(refactor_args)) => {
                if refactor_args.path.is_some() {
                    let path = refactor_args.path.unwrap();
                    let path = Path::new(path.as_str());
                    match fs::canonicalize(path) {
                        Ok(canonical_path) => {
                            if canonical_path.is_dir() {
                                std::env::set_current_dir(&canonical_path)
                                    .expect("Failed to change directory");
                            } else {
                                println!("Can't change current directory to: {}", path.display());
                            }
                        }
                        Err(e) => {
                            println!("Failed to resolve path: {} {}", path.display(), e);
                        }
                    }
                }

                match refactor_args.script_path {
                    Some(script) => {
                        let script_content =
                            fs::read_to_string(script).expect("Failed to read script file");
                        let refactor =
                            codemod::compile(&script_content).expect("Failed to parse script");
                        println!("Running script: {}", refactor);

                        for arg in refactor_args.paths {
                            refactor::process_directory(&refactor, Path::new(arg.as_str()));
                        }
                    }
                    None => {
                        println!("No script defined - using configuration file");
                    }
                }
            }
            Some(Commands::Server(server_args)) => {
                println!("Starting server on port {}", server_args.port);
                language_server::run().await;
            }
            None => {
                println!("No command provided. Use --help for more information.");
            }
        }
    }
}
