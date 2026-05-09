//! Entry point for the `colab` binary.
//!
//! `main` initialises logging and dispatches to [`cli::run`]. The
//! success path returns 0 or 10 (`--check` would-have-changed); errors
//! are mapped to documented exit codes via
//! [`colab_core::Error::exit_code`] and emitted as a single error log
//! line.

mod cli;
mod discover;
mod format;
mod language_server;
mod packs;

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
        Ok(code) => ExitCode::from(code as u8),
        Err(err) => {
            error!("{}", err);
            ExitCode::from(err.exit_code() as u8)
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
