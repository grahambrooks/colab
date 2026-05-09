//! Entry point for the `colab` binary.
//!
//! `main` initialises logging and dispatches to [`cli::run`]. All errors
//! from subcommands surface here as a non-zero exit code with a single
//! human-readable log line.

mod cli;
mod codemod;
mod error;
mod language_server;
mod walker;

use std::io::Write;
use std::process::ExitCode;

use chrono::Local;
use colored::*;
use env_logger::Builder;
use log::{LevelFilter, error};

#[tokio::main]
async fn main() -> ExitCode {
    initialize_logging();

    match cli::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("{}", err);
            ExitCode::FAILURE
        }
    }
}

fn initialize_logging() {
    Builder::new()
        .format(|buf, record| {
            let level_color = match record.level() {
                log::Level::Error => "red",
                log::Level::Warn => "yellow",
                log::Level::Info => "green",
                log::Level::Debug => "blue",
                log::Level::Trace => "magenta",
            };

            writeln!(
                buf,
                "{} [{}] {}",
                Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level().to_string().color(level_color),
                record.args()
            )
        })
        .filter(None, LevelFilter::Info)
        .init();
}
