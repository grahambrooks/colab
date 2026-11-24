//! `--backup <dir>` snapshots and `colab undo`, which restores them.

use super::*;

/// Save a single pre-modification snapshot to `<backup_dir>` with
/// path mirroring. Subsequent calls for the same file are no-ops
/// so the first (truly pre-rule) state is preserved across multi-
/// rule runs.
pub(super) fn write_backup_snapshot(backup_dir: &Path, file: &Path, content: &str) -> Result<()> {
    let target = backup_target(backup_dir, file)?;
    if target.exists() {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
    }
    fs::write(&target, content).map_err(|e| Error::io_at(&target, e))?;
    Ok(())
}

/// The list of files a run created (`create`), one absolute path a line,
/// inside a `--backup` directory: `colab undo` removes them.
pub(super) const CREATED: &str = "colab-created.txt";

/// Note in `<backup_dir>` that the run is about to create `file`.
pub(super) fn record_backup_creation(backup_dir: &Path, file: &Path) -> Result<()> {
    use std::io::Write as _;
    fs::create_dir_all(backup_dir).map_err(|e| Error::io_at(backup_dir, e))?;
    let absolute = std::path::absolute(file).map_err(|e| Error::io_at(file, e))?;
    let list = backup_dir.join(CREATED);
    let mut out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&list)
        .map_err(|e| Error::io_at(&list, e))?;
    writeln!(out, "{}", absolute.display()).map_err(|e| Error::io_at(&list, e))
}

/// Remove the files listed in `<backup>/colab-created.txt`; how many.
fn undo_creations(backup: &Path) -> Result<u64> {
    let list = backup.join(CREATED);
    let text = match fs::read_to_string(&list) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(Error::io_at(&list, e)),
    };
    let mut removed = 0;
    for line in text.lines().filter(|l| !l.is_empty()) {
        let path = Path::new(line);
        match fs::remove_file(path) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io_at(path, e)),
        }
    }
    Ok(removed)
}

/// Given a backup root and an original (possibly relative) path,
/// return the path inside the backup directory. Strips the leading
/// `/` on Unix so the structure mirrors the absolute filesystem
/// layout under the backup root.
pub(super) fn backup_target(backup_dir: &Path, file: &Path) -> Result<PathBuf> {
    let canonical = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let rel = canonical
        .strip_prefix(std::path::MAIN_SEPARATOR.to_string())
        .unwrap_or(&canonical);
    Ok(backup_dir.join(rel))
}

/// Restore every file under `from` to the corresponding location
/// in the filesystem (mirroring the backup-target layout).
pub(super) fn run_undo(from: &Path) -> Result<u8> {
    if !from.is_dir() {
        return Err(Error::Config(format!(
            "backup directory does not exist or is not a directory: {}",
            from.display()
        )));
    }
    // File operations first, so contents land back at original paths.
    let ops = crate::apply::undo_manifest(from)?;
    let created = undo_creations(from)?;
    let mut count = 0u64;
    walk_undo(from, from, &mut count)?;
    if ops > 0 {
        info!("Undid {ops} file operation(s) through git");
    }
    if created > 0 {
        info!("Removed {created} file(s) the run created");
    }
    info!("Restored {} file(s) from {}", count, from.display());
    Ok(0)
}

pub(super) fn walk_undo(root: &Path, current: &Path, count: &mut u64) -> Result<()> {
    let entries = fs::read_dir(current).map_err(|e| Error::io_at(current, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io_at(current, e))?;
        let path = entry.path();
        if path.is_dir() {
            walk_undo(root, &path, count)?;
        } else if current == root
            && (entry.file_name() == crate::apply::MANIFEST || entry.file_name() == CREATED)
        {
            continue;
        } else if path.is_file() {
            let rel = path.strip_prefix(root).map_err(|_| {
                Error::Config(format!(
                    "path {} not under {}",
                    path.display(),
                    root.display()
                ))
            })?;
            let target = Path::new(&std::path::MAIN_SEPARATOR.to_string()).join(rel);
            let content = fs::read_to_string(&path).map_err(|e| Error::io_at(&path, e))?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| Error::io_at(parent, e))?;
            }
            fs::write(&target, content).map_err(|e| Error::io_at(&target, e))?;
            *count += 1;
        }
    }
    Ok(())
}
