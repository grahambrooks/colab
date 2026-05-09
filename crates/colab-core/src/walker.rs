//! Walk filesystem paths and produce [`FileChange`] events.
//!
//! The walker is intentionally agnostic about what callers do with the
//! events: a `--write` run rewrites the file, `--dry-run` reports
//! what would change, `--check` returns a non-zero exit code if any
//! change is needed, and JSON/diff reporters serialize each event.
//!
//! Directory entries are sorted before iteration so output ordering is
//! deterministic across filesystems.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::transformer::CodeTransformer;

/// One file processed by the walker.
///
/// A `FileChange` is produced for every file the transformer
/// considers relevant, regardless of whether the contents actually
/// changed (that is what [`changed`](Self::changed) reports).
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
}

impl FileChange {
    /// `true` when [`apply`](CodeTransformer::apply) produced different
    /// bytes from the original source.
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

/// Walk `path` (file or directory) applying `transformer` to every
/// relevant file. The visitor is invoked for each processed file in
/// sorted order; it decides whether to write, report, or both.
pub fn walk<T, F>(transformer: &T, path: &Path, visit: &mut F) -> Result<()>
where
    T: CodeTransformer,
    F: FnMut(FileChange) -> Result<()>,
{
    if path.is_dir() {
        walk_directory(transformer, path, visit)
    } else if path.is_file() {
        if transformer.is_file_relevant(path) {
            visit_file(transformer, path, visit)?;
        }
        Ok(())
    } else {
        Err(Error::Config(format!(
            "path does not exist or is not a regular file/directory: {}",
            path.display()
        )))
    }
}

fn walk_directory<T, F>(transformer: &T, path: &Path, visit: &mut F) -> Result<()>
where
    T: CodeTransformer,
    F: FnMut(FileChange) -> Result<()>,
{
    let mut children: Vec<PathBuf> = fs::read_dir(path)
        .map_err(|e| Error::io_at(path, e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    children.sort();

    for child in children {
        if child.is_dir() {
            walk_directory(transformer, &child, visit)?;
        } else if transformer.is_file_relevant(&child) {
            visit_file(transformer, &child, visit)?;
        }
    }
    Ok(())
}

fn visit_file<T, F>(transformer: &T, path: &Path, visit: &mut F) -> Result<()>
where
    T: CodeTransformer,
    F: FnMut(FileChange) -> Result<()>,
{
    let before = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
    let after = transformer.apply(&before);
    visit(FileChange {
        path: path.to_path_buf(),
        before,
        after,
    })
}

/// Convenience: rewrite every relevant file in place.
///
/// Mainly useful in tests; production callers wire their own visitor
/// so they can also report progress, format diffs, or honour
/// `--dry-run` / `--check`.
pub fn process_path<T: CodeTransformer>(transformer: &T, path: &Path) -> Result<()> {
    walk(transformer, path, &mut |change: FileChange| {
        if change.changed() {
            fs::write(&change.path, &change.after).map_err(|e| Error::io_at(&change.path, e))?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;

    struct AppendBang;

    impl CodeTransformer for AppendBang {
        fn is_file_relevant(&self, path: &Path) -> bool {
            path.extension().and_then(|s| s.to_str()) == Some("txt")
        }
        fn apply(&self, source: &str) -> String {
            if source.ends_with('!') {
                source.to_string()
            } else {
                format!("{}!", source)
            }
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "colab-walker-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
    }

    #[test]
    fn walk_yields_relevant_files_in_sorted_order() {
        let root = temp_dir("sorted");
        write_file(&root.join("b.txt"), "b");
        write_file(&root.join("a.txt"), "a");
        write_file(&root.join("nested/c.txt"), "c");
        write_file(&root.join("ignore.bin"), "skip");

        let mut seen: Vec<String> = Vec::new();
        walk(&AppendBang, &root, &mut |change| {
            seen.push(
                change
                    .path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            Ok(())
        })
        .unwrap();

        assert_eq!(
            seen,
            vec!["a.txt".to_string(), "b.txt".to_string(), "nested/c.txt".to_string()]
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn process_path_writes_changed_files_only() {
        let root = temp_dir("write");
        let changing = root.join("hello.txt");
        let already_done = root.join("done.txt");
        write_file(&changing, "hello");
        write_file(&already_done, "done!");

        process_path(&AppendBang, &root).unwrap();

        assert_eq!(fs::read_to_string(&changing).unwrap(), "hello!");
        // No-op write shouldn't bump contents.
        assert_eq!(fs::read_to_string(&already_done).unwrap(), "done!");

        fs::remove_dir_all(&root).ok();
    }
}
