pub struct Loader {
    pub config: u32,
}

impl Loader {
    pub fn load(&self) -> u32 {
        let config = self.config;
        config + 1
    }
}

pub fn other(config: u32) -> u32 {
    config * 2
}
