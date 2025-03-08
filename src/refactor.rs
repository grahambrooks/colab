use crate::{codemod};
use log::info;
use std::fs;
use std::path::Path;

pub trait CodeTransformer {
    fn is_file_relevant(&self, path: &Path) -> bool;
    fn apply(&self, source_code: &String) -> String;
}

pub fn process_directory(refactoring: &codemod::Refactoring, path: &Path) {
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("Failed to read directory") {
            let entry = entry.expect("Failed to read directory entry");
            let path = entry.path();
            if path.is_dir() {
                process_directory(refactoring, &path);
            } else if refactoring.is_file_relevant(&path) {
                process_file(refactoring, &path);
            }
        }
    }
}

fn process_file(refactoring: &codemod::Refactoring, path: &Path) {
    info!("Processing {}", path.display());

    let source_code = fs::read_to_string(path).expect("Failed to read file");
    let new_source_code = refactoring.apply(&source_code);
    fs::write(path, new_source_code).expect("Failed to write file");
}
