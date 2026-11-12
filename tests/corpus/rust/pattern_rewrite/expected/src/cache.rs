use std::collections::HashMap;

pub struct Cache {
    entries: HashMap<String, Vec<u8>>,
}

impl Cache {
    pub fn has(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn insert_if_absent(&mut self, key: String, value: Vec<u8>) {
        if !self.entries.contains_key(&key) {
            self.entries.insert(key, value);
        }
    }
}
