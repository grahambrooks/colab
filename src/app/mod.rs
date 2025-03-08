use crate::{codemod, refactor};
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

pub(crate) struct Cli {}

impl Cli {
    pub(crate) fn new() -> Self {
        Cli {}
    }
    pub(crate) fn run(&self) {
        let args = Args::parse();

        if args.path.is_some() {
            let path = args.path.unwrap();
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

        match args.script_path {
            Some(script) => {
                let script_content =
                    fs::read_to_string(script).expect("Failed to read script file");
                let refactor = codemod::compile(&script_content).expect("Failed to parse script");
                println!("Running script: {}", refactor);

                for arg in args.paths {
                    refactor::process_directory(&refactor, Path::new(arg.as_str()));
                }
            }
            None => {
                println!("No script defined - using configuration file");
            }
        }
    }
}
