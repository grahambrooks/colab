//! `colab.toml`: the refactors a repository declares, for `colab fix`.
//!
//! ```toml
//! [run]
//! paths = ["."]                       # where the packs run
//! exclude = ["vendor/**"]             # gitignore-style globs
//! jobs = 8
//! check = "cargo check -q --workspace"   # run after writing; a failure is bisected and reverted
//! tidy = true                         # organize imports and format changed files
//!
//! [formatters]                        # per extension, used by --tidy instead of the language server
//! rs = "rustfmt --edition 2024"       # reads the file on stdin, writes it on stdout
//! ts = "prettier --stdin-filepath {path}"
//!
//! [[packs]]
//! pack = "go/1.18-modern"             # a pack built into colab (`colab pack list`)
//!
//! [[packs]]
//! path = ".colab/packs/house-style.colab"   # relative to colab.toml
//! ```
//!
//! The file is found by walking up from the working directory, stopping
//! at the repository root (the directory holding `.git`). Paths in it are
//! relative to the directory it is in. An unknown key is an error naming
//! the known ones, so a typo is never silently ignored.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use colab_core::{Error, Result};

pub const FILE: &str = "colab.toml";

/// Where a pack comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackRef {
    /// A pack built into colab, by name.
    Builtin(String),
    /// A `.colab` file, relative to `colab.toml`.
    Path(PathBuf),
    /// A pack in a git repository, pinned to a tag or commit: `path` is
    /// the `.colab` file inside it. Fetched into a cache
    /// ([`crate::git_packs`]).
    Git {
        url: String,
        pin: crate::git_packs::Pin,
        path: PathBuf,
    },
}

/// A parsed `colab.toml`.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The directory the file is in; its relative paths start here.
    pub dir: PathBuf,
    pub paths: Vec<PathBuf>,
    pub exclude: Vec<String>,
    pub jobs: Option<usize>,
    pub check: Option<String>,
    pub tidy: bool,
    /// Formatter command by file extension.
    pub formatters: BTreeMap<String, String>,
    pub packs: Vec<PackRef>,
}

/// The nearest `colab.toml` at or above `from`, up to the repository root.
pub fn find_from(from: &Path) -> Option<PathBuf> {
    let mut dir = from.to_path_buf();
    loop {
        let candidate = dir.join(FILE);
        if candidate.is_file() {
            return Some(candidate);
        }
        if dir.join(".git").exists() || !dir.pop() {
            return None;
        }
    }
}

/// The nearest `colab.toml` above the working directory, parsed, if any.
pub fn find() -> Result<Option<Config>> {
    let cwd = std::env::current_dir().map_err(|e| Error::io_at(Path::new("."), e))?;
    find_from(&cwd).map(|p| load(&p)).transpose()
}

fn bad(path: &Path, why: impl std::fmt::Display) -> Error {
    Error::Config(format!("{}: {why}", path.display()))
}

fn strings(path: &Path, key: &str, item: &toml_edit::Item) -> Result<Vec<String>> {
    let array = item
        .as_array()
        .ok_or_else(|| bad(path, format!("`{key}` is a list of strings")))?;
    array
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| bad(path, format!("`{key}` is a list of strings")))
        })
        .collect()
}

/// Read and check `path`.
pub fn load(path: &Path) -> Result<Config> {
    let text = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
    parse(path, &text)
}

/// Parse `text`, the contents of `path`.
pub fn parse(path: &Path, text: &str) -> Result<Config> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| bad(path, e))?;
    let mut config = Config {
        dir: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
        ..Default::default()
    };
    for (key, item) in doc.iter() {
        match key {
            "run" => {
                let table = item
                    .as_table()
                    .ok_or_else(|| bad(path, "`[run]` is a table"))?;
                for (key, item) in table.iter() {
                    match key {
                        "paths" => {
                            config.paths = strings(path, "run.paths", item)?
                                .into_iter()
                                .map(PathBuf::from)
                                .collect()
                        }
                        "exclude" => config.exclude = strings(path, "run.exclude", item)?,
                        "jobs" => {
                            config.jobs = Some(
                                item.as_integer()
                                    .filter(|n| *n > 0)
                                    .ok_or_else(|| bad(path, "`run.jobs` is a positive number"))?
                                    as usize,
                            )
                        }
                        "check" => {
                            config.check = Some(
                                item.as_str()
                                    .ok_or_else(|| bad(path, "`run.check` is a command"))?
                                    .to_string(),
                            )
                        }
                        "tidy" => {
                            config.tidy = item
                                .as_bool()
                                .ok_or_else(|| bad(path, "`run.tidy` is true or false"))?
                        }
                        other => {
                            return Err(bad(
                                path,
                                format!(
                                    "unknown key `run.{other}`; [run] takes paths, exclude, jobs, check, tidy"
                                ),
                            ));
                        }
                    }
                }
            }
            "formatters" => {
                let table = item
                    .as_table()
                    .ok_or_else(|| bad(path, "`[formatters]` is a table"))?;
                for (ext, command) in table.iter() {
                    let command = command
                        .as_str()
                        .ok_or_else(|| bad(path, format!("`formatters.{ext}` is a command")))?;
                    config
                        .formatters
                        .insert(ext.to_string(), command.to_string());
                }
            }
            "packs" => {
                let tables = item
                    .as_array_of_tables()
                    .ok_or_else(|| bad(path, "`packs` is a list of `[[packs]]` tables"))?;
                for table in tables.iter() {
                    let text = |key: &str| table.get(key).and_then(|v| v.as_str());
                    let builtin = text("pack");
                    let file = text("path");
                    const KEYS: [&str; 5] = ["pack", "path", "git", "tag", "rev"];
                    if let Some(other) = table.iter().map(|(k, _)| k).find(|k| !KEYS.contains(k)) {
                        return Err(bad(
                            path,
                            format!(
                                "unknown key `packs.{other}`; a pack has `pack`, `path`, or `git` with `tag` or `rev` and `path`"
                            ),
                        ));
                    }
                    if let Some(url) = text("git") {
                        let pin = match (text("tag"), text("rev")) {
                            (Some(tag), None) => crate::git_packs::Pin::Tag(tag.to_string()),
                            (None, Some(rev)) => crate::git_packs::Pin::Rev(rev.to_string()),
                            _ => {
                                return Err(bad(
                                    path,
                                    format!(
                                        "the git pack `{url}` needs one pin: `tag = \"<tag>\"` or `rev = \"<commit>\"`"
                                    ),
                                ));
                            }
                        };
                        let Some(file) = file.filter(|_| builtin.is_none()) else {
                            return Err(bad(
                                path,
                                format!(
                                    "the git pack `{url}` needs `path = \"<the .colab file in it>\"`"
                                ),
                            ));
                        };
                        config.packs.push(PackRef::Git {
                            url: url.to_string(),
                            pin,
                            path: PathBuf::from(file),
                        });
                        continue;
                    }
                    if text("tag").is_some() || text("rev").is_some() {
                        return Err(bad(
                            path,
                            "`tag` and `rev` pin a `git` pack; this one has no `git`",
                        ));
                    }
                    config.packs.push(match (builtin, file) {
                        (Some(name), None) => PackRef::Builtin(name.to_string()),
                        (None, Some(file)) => PackRef::Path(PathBuf::from(file)),
                        _ => {
                            return Err(bad(
                                path,
                                "each `[[packs]]` has exactly one of `pack = \"<built-in>\"` or `path = \"<file>\"`",
                            ));
                        }
                    });
                }
            }
            other => {
                return Err(bad(
                    path,
                    format!(
                        "unknown section `{other}`; colab.toml has [run], [formatters] and [[packs]]"
                    ),
                ));
            }
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_git_pack_takes_one_pin_and_a_path() {
        let ok = parse(
            Path::new("colab.toml"),
            "[[packs]]\ngit = \"https://example.test/p.git\"\ntag = \"v1\"\npath = \"go/x.colab\"\n",
        )
        .unwrap();
        assert_eq!(
            ok.packs,
            [PackRef::Git {
                url: "https://example.test/p.git".into(),
                pin: crate::git_packs::Pin::Tag("v1".into()),
                path: "go/x.colab".into(),
            }]
        );
        let err = |text: &str| {
            parse(Path::new("colab.toml"), text)
                .unwrap_err()
                .to_string()
        };
        assert!(err("[[packs]]\ngit = \"u\"\npath = \"x\"\n").contains("needs one pin"));
        assert!(
            err("[[packs]]\ngit = \"u\"\ntag = \"a\"\nrev = \"b\"\npath = \"x\"\n")
                .contains("needs one pin")
        );
        assert!(err("[[packs]]\ngit = \"u\"\ntag = \"a\"\n").contains("needs `path"));
        assert!(err("[[packs]]\npath = \"x\"\ntag = \"a\"\n").contains("has no `git`"));
    }

    #[test]
    fn parses_the_documented_shape() {
        let text = "[run]\npaths = [\"src\"]\nexclude = [\"vendor/**\"]\njobs = 4\ncheck = \"cargo check\"\ntidy = true\n\n[formatters]\nrs = \"rustfmt\"\n\n[[packs]]\npack = \"go/1.18-modern\"\n\n[[packs]]\npath = \".colab/packs/x.colab\"\n";
        let c = parse(Path::new("/repo/colab.toml"), text).unwrap();
        assert_eq!(c.dir, Path::new("/repo"));
        assert_eq!(c.paths, [PathBuf::from("src")]);
        assert_eq!(c.jobs, Some(4));
        assert!(c.tidy);
        assert_eq!(c.formatters["rs"], "rustfmt");
        assert_eq!(
            c.packs,
            [
                PackRef::Builtin("go/1.18-modern".into()),
                PackRef::Path(".colab/packs/x.colab".into())
            ]
        );
    }

    #[test]
    fn mistakes_name_what_is_allowed() {
        let p = Path::new("colab.toml");
        let e = parse(p, "[run]\npath = [\".\"]\n").unwrap_err().to_string();
        assert!(
            e.contains("unknown key `run.path`; [run] takes paths"),
            "{e}"
        );
        let e = parse(p, "[[packs]]\npack = \"a\"\npath = \"b\"\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("exactly one of"), "{e}");
        assert!(
            parse(p, "[pack]\n")
                .unwrap_err()
                .to_string()
                .contains("unknown section")
        );
    }
}
