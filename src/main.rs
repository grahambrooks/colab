mod codemod;
mod go;
mod refactor;
mod app;

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
CLI for refactoriing based code modifications 'codemods'.

"#
)]
struct Args {
    #[arg(long, help = "Script to run against the codebase")]
    script: Option<String>,
    #[arg(
        short = 'C',
        help = "Change working directoryPaths to the files or directories to process",
        default_value = "."
    )]
    path: Option<String>,
    paths: Vec<String>,
}

fn main() {
    let app = app::Cli::new();
    app.run();
    let args = Args::parse();

    if args.path.is_some() {
        let path = args.path.unwrap();
        let path = Path::new(path.as_str());
        match fs::canonicalize(path) {
            Ok(canonical_path) => {
                if canonical_path.is_dir() {
                    std::env::set_current_dir(&canonical_path).expect("Failed to change directory");
                } else {
                    println!("Can't change current directory to: {}", path.display());
                }
            }
            Err(e) => {
                println!("Failed to resolve path: {} {}", path.display(), e);
            }
        }
    }

    match args.script {
        Some(script) => {
            let script_content = fs::read_to_string(script).expect("Failed to read script file");
                        let refactor = codemod::compile(&script_content).expect("Failed to parse script");
            println!("Running script: {}", refactor);
            run(args.paths, refactor);
            match refactor {
                codemod::Refactoring { .. } => {}
            }
        }
        None => {
            println!("No script defined - using configuration file");
        }
    }
}

fn run(paths: Vec<String>, refactor: codemod::Refactoring) {
    for arg in paths {
        refactor::process_directory(&refactor, Path::new(arg.as_str()));
    }
}
