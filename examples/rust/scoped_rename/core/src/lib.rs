/// The core crate's configuration.
pub struct Config {
    pub verbose: bool,
}

impl Config {
    pub fn new() -> Self {
        Config { verbose: false }
    }
}
