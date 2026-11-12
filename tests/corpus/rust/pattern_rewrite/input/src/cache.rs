use std::collections::HashMap;

pub struct Cache {
    entries: HashMap<String, Vec<u8>>,
}

impl Cache {
    pub fn has(&self, key: &str) -> bool {
        dbg!(key);
        self.entries.get(key).is_some()
    }

    pub fn insert_if_absent(&mut self, key: String, value: Vec<u8>) {
        if !self.entries.get(&key).is_some() {
            self.entries.insert(key, value);
        }
    }
}
