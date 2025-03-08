mod app;
mod codemod;
mod go;
mod refactor;
mod language_server;

#[tokio::main]
async fn main() {
    let app = app::Cli::new();
    app.run().await;
}
