//! Recursively apply a [`CodeTransformer`] to files on disk.
//!
//! The walker is intentionally agnostic about which transformer it runs:
//! it inspects each candidate file with
//! [`CodeTransformer::is_file_relevant`] and only rewrites files whose
//! contents actually change. Errors are wrapped with the offending path so
//! callers can produce useful diagnostics.

use std::fs;
use std::path::Path;

use log::{debug, info};

use crate::codemod::CodeTransformer;
use crate::error::{Error, Result};

/// Apply `transformer` to `path`. Directories are walked recursively;
/// files are processed when [`CodeTransformer::is_file_relevant`] is `true`.
pub fn process_path<T: CodeTransformer>(transformer: &T, path: &Path) -> Result<()> {
    if path.is_dir() {
        process_directory(transformer, path)
    } else if path.is_file() {
        if transformer.is_file_relevant(path) {
            process_file(transformer, path)?;
        }
        Ok(())
    } else {
        Err(Error::Config(format!(
            "path does not exist or is not a regular file/directory: {}",
            path.display()
        )))
    }
}

fn process_directory<T: CodeTransformer>(transformer: &T, path: &Path) -> Result<()> {
    let entries = fs::read_dir(path).map_err(|e| Error::io_at(path, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io_at(path, e))?;
        let child = entry.path();
        if child.is_dir() {
            process_directory(transformer, &child)?;
        } else if transformer.is_file_relevant(&child) {
            process_file(transformer, &child)?;
        }
    }
    Ok(())
}

fn process_file<T: CodeTransformer>(transformer: &T, path: &Path) -> Result<()> {
    info!("Processing {}", path.display());

    let source = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
    let updated = transformer.apply(&source);

    if updated == source {
        debug!("No changes for {}", path.display());
        return Ok(());
    }

    fs::write(path, updated).map_err(|e| Error::io_at(path, e))
}
