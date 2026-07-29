pub struct CoreConfig {
    pub verbose: bool,
}

impl CoreConfig {
    pub fn new() -> Self {
        CoreConfig { verbose: false }
    }
}
