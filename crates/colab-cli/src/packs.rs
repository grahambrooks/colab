//! Pack catalog: discover `.codemod` files in well-known locations.
//!
//! Lookup paths, in order:
//!
//! 1. `<repo>/.colab/packs/` — the project-local pack directory,
//!    discovered by walking up from the current directory looking
//!    for a `.git` marker.
//! 2. `$HOME/.colab/packs/` — the user-global pack directory.
//!
//! Returned entries are sorted by path so two invocations against
//! the same filesystem produce identical output. Remote registries
//! and `colab pack install` are deferred (see development plan
//! M11 follow-ups).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Build the JSON document `colab pack list` emits.
pub fn list() -> Value {
    let mut packs = Vec::new();
    if let Some(repo_dir) = repo_packs_dir() {
        scan(&repo_dir, "repo", &mut packs);
    }
    if let Some(user_dir) = user_packs_dir() {
        scan(&user_dir, "user", &mut packs);
    }
    packs.sort_by(|a, b| {
        let pa = a["path"].as_str().unwrap_or("");
        let pb = b["path"].as_str().unwrap_or("");
        pa.cmp(pb)
    });
    json!({ "packs": packs })
}

fn user_packs_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join(".colab").join("packs");
    dir.is_dir().then_some(dir)
}

/// Walk up from the current directory looking for the nearest
/// `.git` (a directory or a file — the latter is a worktree's
/// gitfile). The pack directory lives at `<repo>/.colab/packs/`.
fn repo_packs_dir() -> Option<PathBuf> {
    let mut current = std::env::current_dir().ok()?;
    loop {
        if current.join(".git").exists() {
            let dir = current.join(".colab").join("packs");
            return dir.is_dir().then_some(dir);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn scan(dir: &Path, source: &str, out: &mut Vec<Value>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("codemod") {
            continue;
        }
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        out.push(json!({
            "name": name,
            "path": path.to_string_lossy(),
            "source": source,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "colab-packs-{}-{}-{}",
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

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
    }

    #[test]
    fn scan_picks_only_codemod_files() {
        let dir = temp_dir("scan");
        write(&dir.join("first.codemod"), "refactor \"x\" {}");
        write(&dir.join("second.codemod"), "refactor \"y\" {}");
        write(&dir.join("README.md"), "not a pack");

        let mut packs = Vec::new();
        scan(&dir, "user", &mut packs);
        let names: Vec<&str> = packs
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"first"));
        assert!(names.contains(&"second"));
        assert!(!names.iter().any(|n| n == &"README"));

        for pack in &packs {
            assert_eq!(pack["source"], "user");
        }

        fs::remove_dir_all(&dir).ok();
    }
}
