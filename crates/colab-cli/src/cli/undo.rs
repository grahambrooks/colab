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
    let mut count = 0u64;
    walk_undo(from, from, &mut count)?;
    if ops > 0 {
        info!("Undid {ops} file operation(s) through git");
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
        } else if current == root && entry.file_name() == crate::apply::MANIFEST {
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
