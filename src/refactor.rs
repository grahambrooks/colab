use crate::{codemod, go};
use std::fs;
use std::path::Path;
pub fn process_directory(config: &codemod::Config, path: &Path) {
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("Failed to read directory") {
            let entry = entry.expect("Failed to read directory entry");
            let path = entry.path();
            if path.is_dir() {
                process_directory(config, &path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("go") {
                process_file(config, &path);
            }
        }
    }
}

fn process_file(config: &codemod::Config, path: &Path) {
    println!("Processing {}", path.display());
    let replacement = &config.replace.go_module;
    let source_code = fs::read_to_string(path).expect("Failed to read file");

    let new_source_code = go::imports::rename(&replacement, &source_code);

    fs::write(path, new_source_code).expect("Failed to write file");
}
