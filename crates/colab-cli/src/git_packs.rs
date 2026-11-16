//! Packs shared as git sources: `[[packs]] git = "<url>", tag = "<tag>",
//! path = "<pack>"` in `colab.toml`.
//!
//! A pack is pinned (a tag or a commit, never a branch that moves), so a
//! repository's `colab fix` means the same thing tomorrow. Each pin is
//! fetched once, shallow, into a cache (`COLAB_CACHE_DIR`, else
//! `$XDG_CACHE_HOME/colab`, else `~/.cache/colab`), built under a
//! temporary name and moved into place only when complete, so an
//! interrupted fetch is never used. Before a fetched pack runs, its own
//! `<pack>.tests/` cases must pass (`colab test`); a pack that fails them
//! is refused, naming the failing case. There is no registry: a URL and a
//! pin are the whole address.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use colab_core::{BackendRegistry, Error, Result};

/// What a git pack is pinned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pin {
    Tag(String),
    /// A commit id.
    Rev(String),
}

impl Pin {
    fn name(&self) -> &str {
        match self {
            Pin::Tag(t) | Pin::Rev(t) => t,
        }
    }
}

impl std::fmt::Display for Pin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Pin::Tag(t) => write!(f, "tag {t}"),
            Pin::Rev(r) => write!(f, "rev {r}"),
        }
    }
}

/// Where fetched packs are kept.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("COLAB_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("colab")
}

const COMPLETE: &str = ".colab-fetched";

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| Error::Io {
            path: None,
            source: e,
        })?;
    if !out.status.success() {
        return Err(Error::Git(format!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The checkout of `url` at `pin`, fetching it into the cache the first
/// time.
pub fn checkout(url: &str, pin: &Pin) -> Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    let safe: String = pin
        .name()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let packs = cache_dir().join("packs");
    let dir = packs.join(format!("{:016x}-{safe}", hasher.finish()));
    if dir.join(COMPLETE).is_file() {
        return Ok(dir);
    }
    std::fs::create_dir_all(&packs).map_err(|e| Error::io_at(&packs, e))?;
    let partial = packs.join(format!(
        ".partial-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&partial).map_err(|e| Error::io_at(&partial, e))?;
    let fetched = (|| {
        git(&partial, &["init", "-q"])?;
        let refspec = match pin {
            Pin::Tag(tag) => format!("refs/tags/{tag}"),
            Pin::Rev(rev) => rev.clone(),
        };
        git(&partial, &["fetch", "-q", "--depth", "1", url, &refspec])
            .map_err(|e| Error::Git(format!("fetching the pack {url} at {pin}: {e}")))?;
        git(
            &partial,
            &[
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "-q",
                "FETCH_HEAD",
            ],
        )?;
        let commit = git(&partial, &["rev-parse", "HEAD"])?;
        std::fs::write(partial.join(COMPLETE), format!("{url} {pin} {commit}\n"))
            .map_err(|e| Error::io_at(&partial, e))
    })();
    if let Err(e) = fetched {
        std::fs::remove_dir_all(&partial).ok();
        return Err(e);
    }
    match std::fs::rename(&partial, &dir) {
        Ok(()) => Ok(dir),
        // Another run fetched it meanwhile; its copy is as good.
        Err(_) if dir.join(COMPLETE).is_file() => {
            std::fs::remove_dir_all(&partial).ok();
            Ok(dir)
        }
        Err(e) => Err(Error::io_at(&dir, e)),
    }
}

/// The pack file `path` in `url` at `pin`, fetched, and its own cases
/// passed.
pub fn resolve(url: &str, pin: &Pin, path: &Path, backends: &BackendRegistry) -> Result<PathBuf> {
    if path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(Error::Config(format!(
            "the git pack {url}: `path = \"{}\"` must be a path inside the repository",
            path.display()
        )));
    }
    let checkout = checkout(url, pin)?;
    let pack = checkout.join(path);
    if !pack.is_file() {
        return Err(Error::Config(format!(
            "the git pack {url} at {pin} has no {}",
            path.display()
        )));
    }
    if pack.with_extension("tests").is_dir() {
        let failing = crate::pack_test::failing_cases(&pack, backends)?;
        if !failing.is_empty() {
            return Err(Error::Config(format!(
                "the git pack {url} at {pin} ({}) fails its own tests, so it is not run: {}",
                path.display(),
                failing.join("; ")
            )));
        }
    }
    Ok(pack)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_outside_the_repository_is_refused() {
        let e = resolve(
            "file:///nowhere",
            &Pin::Tag("v1".into()),
            Path::new("../x.colab"),
            &colab_backends::registry(),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("inside the repository"), "{e}");
    }
}
