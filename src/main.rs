mod app;
mod codemod;
mod refactor;
mod language_server;

use env_logger::Builder;
use log::LevelFilter;
use std::io::Write;
use colored::*;
use chrono::Local;

#[tokio::main]
async fn main() {
    initialize_logging();

    let app = app::Cli::new();
    app.run().await;
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
        .filter(None, LevelFilter::Info) // Set default log level to Info
        .init();
}
