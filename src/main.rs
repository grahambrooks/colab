mod config;
mod refactor;
mod integration_test;

use clap::Parser;
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
CLI for refactoriing

"#
)]
struct Args {
    #[arg(
        long,
        help = "Truncate the JSON line output for each line. Useful for previewing the output when scanning a large number of files"
    )]
    config: Option<String>,
    paths: Vec<String>,
}

fn main() {
    let args = Args::parse();

    let config_path = args.config.unwrap_or("config.yaml".to_string());

    let app_config: config::Config = config::read_config(config_path).unwrap();

    let go_language = tree_sitter_go::LANGUAGE.into();
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&go_language)
        .expect("Error loading Go grammar");

    for arg in args.paths {
        refactor::process_directory(&mut parser, &app_config, Path::new(arg.as_str()));
    }
}
