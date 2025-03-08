mod app;
mod codemod;
mod go;
mod refactor;

fn main() {
    let app = app::Cli::new();
    app.run();
}
