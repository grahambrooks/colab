mod codemod;
mod config;
mod go;
mod integration_test;
mod refactor;

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
    #[arg(
        long,
        help = "Truncate the JSON line output for each line. Useful for previewing the output when scanning a large number of files"
    )]
    config: Option<String>,
    #[arg(long, help = "Script to run against the codebase")]
    script: Option<String>,
    paths: Vec<String>,
}

fn main() {
    let args = Args::parse();

    match args.script {
        Some(script) => {
            let script_content = fs::read_to_string(script).expect("Failed to read script file");
            codemod::parse(&script_content).expect("Failed to parse script");
        }
        None => {
            println!("No script defined - using configuration file");
        }
    }

    let config_path = args.config.unwrap_or("config.yaml".to_string());

    run(args.paths, config_path);
}

fn run(paths: Vec<String>, config_path: String) {
    let app_config: config::Config = config::read_config(config_path).unwrap();

    for arg in paths {
        refactor::process_directory(&app_config, Path::new(arg.as_str()));
    }
}
