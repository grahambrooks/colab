/// A different `Config` that happens to share the name. Must not be
/// renamed: it is not the type the codemod is about.
pub struct Config {
    pub port: u16,
}

fn main() {
    let _ = Config { port: 8080 };
}
