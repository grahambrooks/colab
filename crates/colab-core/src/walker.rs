//! Walk filesystem paths and produce [`FileChange`] events.
//!
//! The walker is intentionally agnostic about what callers do with
//! the events: a `--write` run rewrites the file, `--dry-run`
//! reports what would change, `--check` returns a non-zero exit
//! code if any change is needed, and JSON/diff reporters serialize
//! each event.
//!
//! Filtering is layered on top via [`WalkOptions`]:
//!
//! - **Include / exclude globs** (gitignore syntax) — see
//!   `--include` / `--exclude` in the CLI.
//! - **`.gitignore` awareness** — default-on. Honours `.gitignore`,
//!   `.git/info/exclude`, the global ignore file, and skips
//!   hidden files. Disable with [`WalkOptions::respect_gitignore`].
//! - **Symlink-following** — off by default.
//!
//! Output ordering is deterministic: directory entries are walked
//! in path-sorted order, then parallel processing preserves that
//! order via `rayon::par_iter().collect()`. Two runs of the same
//! script against the same tree emit identical event streams
//! regardless of `--jobs`.
//!
//! Files are processed in chunks of [`CHUNK_SIZE`] so even a
//! 100k-file run only holds a bounded number of [`FileChange`]
//! values in memory at once.

use std::fs;
use std::path::{Path, PathBuf};

use globset::GlobBuilder;
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use log::{debug, info};
use rayon::ThreadPool;
use rayon::prelude::*;

use crate::error::{Error, Result};
use crate::transformer::CodeTransformer;

/// Files-per-batch when processing in parallel. Caps memory at
/// roughly `CHUNK_SIZE × max_file_size` so even 100k+ file trees
/// stream through without holding everything in RAM.
const CHUNK_SIZE: usize = 256;

/// Build a rayon thread pool sized for this walk. Each call to
/// [`walk_with`] gets its own pool so back-to-back invocations with
/// different `jobs` settings honour the new value (the rayon
/// global pool can only be sized once per process). The pool's
/// workers shut down when it's dropped at the end of the walk.
fn build_pool(jobs: Option<usize>) -> Result<ThreadPool> {
    let n = jobs.filter(|j| *j > 0).unwrap_or_else(num_cpus::get);
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .build()
        .map_err(|e| Error::Config(format!("could not build thread pool: {}", e)))
}

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
    /// `true` when [`apply`](CodeTransformer::apply) produced
    /// different bytes from the original source.
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

/// Filtering options applied when walking a directory tree.
///
/// All defaults match the CLI's behaviour with no flags: respect
/// `.gitignore`, no glob filters, no symlink following, parallel
/// processing across `num_cpus` threads.
#[derive(Debug, Clone)]
pub struct WalkOptions {
    /// Whitelist patterns. If non-empty, only files matching at
    /// least one pattern are visited.
    pub include: Vec<String>,
    /// Blacklist patterns. Always exclude matching files.
    pub exclude: Vec<String>,
    /// Honour `.gitignore` / `.ignore` / hidden-file rules.
    pub respect_gitignore: bool,
    /// Follow symlinks during the walk.
    pub follow_symlinks: bool,
    /// Worker thread count for parallel processing. `None` →
    /// `num_cpus`; `Some(1)` forces sequential. Only the first
    /// invocation per process actually sizes the global rayon pool;
    /// subsequent values are silently ignored.
    pub jobs: Option<usize>,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            exclude: Vec::new(),
            respect_gitignore: true,
            follow_symlinks: false,
            jobs: None,
        }
    }
}

/// Walk `path` (file or directory) applying `transformer` to every
/// relevant file. Equivalent to [`walk_with`] with default options.
pub fn walk<T, F>(transformer: &T, path: &Path, visit: &mut F) -> Result<()>
where
    T: CodeTransformer + Sync,
    F: FnMut(FileChange) -> Result<()>,
{
    walk_with(transformer, path, &WalkOptions::default(), visit)
}

/// Walk `path` honouring `opts`. The visitor is invoked for each
/// processed file in path-sorted order; it decides whether to
/// write, report, or both.
///
/// Internally the walker runs in three phases:
///
/// 1. **Discovery.** Sequential `ignore::WalkBuilder` collects
///    every eligible file path (no IO read).
/// 2. **Process.** Files are processed in chunks of
///    [`CHUNK_SIZE`] via `rayon::par_iter`. Each worker reads its
///    file from disk, runs `transformer.apply`, and produces a
///    [`FileChange`].
/// 3. **Deliver.** Each chunk is delivered to the visitor in path
///    order — `par_iter().collect()` preserves the input order so
///    determinism is free.
pub fn walk_with<T, F>(
    transformer: &T,
    path: &Path,
    opts: &WalkOptions,
    visit: &mut F,
) -> Result<()>
where
    T: CodeTransformer + Sync,
    F: FnMut(FileChange) -> Result<()>,
{
    if !path.exists() {
        return Err(Error::Config(format!(
            "path does not exist: {}",
            path.display()
        )));
    }

    if path.is_file() {
        // Directly-supplied files bypass `.gitignore` (the user
        // explicitly chose them). Glob filters still apply.
        if !file_matches_filters(path, opts)? {
            return Ok(());
        }
        if transformer.is_file_relevant(path) {
            visit_file(transformer, path, visit)?;
        }
        return Ok(());
    }

    let pool = build_pool(opts.jobs)?;
    let paths = collect_paths(transformer, path, opts)?;
    process_paths_parallel(transformer, &paths, &pool, visit)
}

/// Discovery phase: walk the tree synchronously, applying ignore
/// rules and globs, returning the relevant file paths in sorted
/// order.
fn collect_paths<T: CodeTransformer>(
    transformer: &T,
    root: &Path,
    opts: &WalkOptions,
) -> Result<Vec<PathBuf>> {
    let mut builder = WalkBuilder::new(root);
    builder
        .git_ignore(opts.respect_gitignore)
        .git_global(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .ignore(opts.respect_gitignore)
        .hidden(opts.respect_gitignore)
        .follow_links(opts.follow_symlinks)
        .sort_by_file_path(|a, b| a.cmp(b));

    if !opts.include.is_empty() || !opts.exclude.is_empty() {
        let mut overrides = OverrideBuilder::new(root);
        for pattern in &opts.include {
            overrides
                .add(pattern)
                .map_err(|e| Error::Config(format!("invalid --include glob `{}`: {}", pattern, e)))?;
        }
        for pattern in &opts.exclude {
            overrides
                .add(&format!("!{}", pattern))
                .map_err(|e| {
                    Error::Config(format!("invalid --exclude glob `{}`: {}", pattern, e))
                })?;
        }
        let overrides = overrides
            .build()
            .map_err(|e| Error::Config(format!("could not build glob overrides: {}", e)))?;
        builder.overrides(overrides);
    }

    let mut paths = Vec::new();
    for result in builder.build() {
        let entry = result.map_err(|e| Error::Config(format!("walk error: {}", e)))?;
        let entry_path = entry.path();
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if !transformer.is_file_relevant(entry_path) {
            continue;
        }
        paths.push(entry_path.to_path_buf());
    }
    Ok(paths)
}

/// Process / deliver phase: chunked parallel read + apply (on the
/// supplied pool), then sequential delivery to the visitor in
/// input (sorted) order.
fn process_paths_parallel<T, F>(
    transformer: &T,
    paths: &[PathBuf],
    pool: &ThreadPool,
    visit: &mut F,
) -> Result<()>
where
    T: CodeTransformer + Sync,
    F: FnMut(FileChange) -> Result<()>,
{
    for chunk in paths.chunks(CHUNK_SIZE) {
        let changes: Vec<Result<FileChange>> = pool.install(|| {
            chunk
                .par_iter()
                .map(|p| compute_change(transformer, p))
                .collect()
        });
        for change in changes {
            let change = change?;
            if change.before == change.after {
                debug!("No changes for {}", change.path.display());
            }
            info!("Processing {}", change.path.display());
            visit(change)?;
        }
    }
    Ok(())
}

fn compute_change<T: CodeTransformer>(transformer: &T, path: &Path) -> Result<FileChange> {
    let before = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
    let after = transformer.apply(&before);
    Ok(FileChange {
        path: path.to_path_buf(),
        before,
        after,
    })
}

/// True if `path` passes any explicit `include` and is not blocked
/// by `exclude`. `.gitignore` does not apply to directly-supplied
/// files, only to tree walks.
fn file_matches_filters(path: &Path, opts: &WalkOptions) -> Result<bool> {
    if !opts.include.is_empty() {
        let mut any = false;
        for pattern in &opts.include {
            if path_matches_glob(path, pattern)? {
                any = true;
                break;
            }
        }
        if !any {
            return Ok(false);
        }
    }
    for pattern in &opts.exclude {
        if path_matches_glob(path, pattern)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn path_matches_glob(path: &Path, pattern: &str) -> Result<bool> {
    let glob = GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()
        .map_err(|e| Error::Config(format!("invalid glob `{}`: {}", pattern, e)))?;
    Ok(glob.compile_matcher().is_match(path))
}

fn visit_file<T, F>(transformer: &T, path: &Path, visit: &mut F) -> Result<()>
where
    T: CodeTransformer,
    F: FnMut(FileChange) -> Result<()>,
{
    info!("Processing {}", path.display());
    let before = fs::read_to_string(path).map_err(|e| Error::io_at(path, e))?;
    let after = transformer.apply(&before);
    if before == after {
        debug!("No changes for {}", path.display());
    }
    visit(FileChange {
        path: path.to_path_buf(),
        before,
        after,
    })
}

/// Convenience: rewrite every relevant file in place.
///
/// Used by tests and by callers that don't need any of the
/// reporter / dry-run / check machinery.
pub fn process_path<T: CodeTransformer + Sync>(transformer: &T, path: &Path) -> Result<()> {
    walk(transformer, path, &mut |change: FileChange| {
        if change.changed() {
            fs::write(&change.path, &change.after)
                .map_err(|e| Error::io_at(&change.path, e))?;
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

    fn collect(transformer: &AppendBang, root: &Path, opts: &WalkOptions) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        walk_with(transformer, root, opts, &mut |change| {
            seen.push(
                change
                    .path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            Ok(())
        })
        .unwrap();
        seen
    }

    #[test]
    fn walk_yields_relevant_files_in_sorted_order() {
        let root = temp_dir("sorted");
        write_file(&root.join("b.txt"), "b");
        write_file(&root.join("a.txt"), "a");
        write_file(&root.join("nested/c.txt"), "c");
        write_file(&root.join("ignore.bin"), "skip");

        let seen = collect(&AppendBang, &root, &WalkOptions::default());
        assert_eq!(
            seen,
            vec![
                "a.txt".to_string(),
                "b.txt".to_string(),
                "nested/c.txt".to_string(),
            ]
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
        assert_eq!(fs::read_to_string(&already_done).unwrap(), "done!");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn include_glob_restricts_visited_files() {
        let root = temp_dir("include");
        write_file(&root.join("a.txt"), "a");
        write_file(&root.join("nested/b.txt"), "b");
        write_file(&root.join("nested/c.txt"), "c");

        let opts = WalkOptions {
            include: vec!["nested/**".into()],
            ..Default::default()
        };
        let seen = collect(&AppendBang, &root, &opts);
        assert!(!seen.contains(&"a.txt".to_string()));
        assert!(seen.contains(&"nested/b.txt".to_string()));
        assert!(seen.contains(&"nested/c.txt".to_string()));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn exclude_glob_drops_matching_files() {
        let root = temp_dir("exclude");
        write_file(&root.join("a.txt"), "a");
        write_file(&root.join("vendor/b.txt"), "b");
        write_file(&root.join("nested/c.txt"), "c");

        let opts = WalkOptions {
            exclude: vec!["vendor/**".into()],
            ..Default::default()
        };
        let seen = collect(&AppendBang, &root, &opts);
        assert!(seen.contains(&"a.txt".to_string()));
        assert!(!seen.iter().any(|p| p.starts_with("vendor")));
        assert!(seen.contains(&"nested/c.txt".to_string()));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn gitignore_is_honoured_by_default() {
        let root = temp_dir("gitignore-on");
        // The `ignore` crate needs a .git directory to recognise
        // this as a git project root.
        fs::create_dir_all(root.join(".git")).unwrap();
        write_file(&root.join(".gitignore"), "vendor/\n");
        write_file(&root.join("a.txt"), "a");
        write_file(&root.join("vendor/b.txt"), "b");

        let seen = collect(&AppendBang, &root, &WalkOptions::default());
        assert!(seen.contains(&"a.txt".to_string()));
        assert!(!seen.iter().any(|p| p.starts_with("vendor")));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_ignore_visits_gitignored_files() {
        let root = temp_dir("gitignore-off");
        fs::create_dir_all(root.join(".git")).unwrap();
        write_file(&root.join(".gitignore"), "vendor/\n");
        write_file(&root.join("a.txt"), "a");
        write_file(&root.join("vendor/b.txt"), "b");

        let opts = WalkOptions {
            respect_gitignore: false,
            ..Default::default()
        };
        let seen = collect(&AppendBang, &root, &opts);
        assert!(seen.contains(&"a.txt".to_string()));
        assert!(seen.contains(&"vendor/b.txt".to_string()));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn parallel_walk_is_deterministic() {
        // Drive a wider tree through the walker many times; the
        // event order must not depend on rayon scheduling.
        let root = temp_dir("parallel-determinism");
        for i in 0..40 {
            write_file(&root.join(format!("dir{:02}/{:02}.txt", i % 5, i)), "x");
        }

        let mut runs: Vec<Vec<String>> = Vec::new();
        for _ in 0..6 {
            runs.push(collect(&AppendBang, &root, &WalkOptions::default()));
        }
        let first = &runs[0];
        for run in &runs[1..] {
            assert_eq!(run, first);
        }
        // And the order is path-sorted (lexicographic).
        let mut sorted = first.clone();
        sorted.sort();
        assert_eq!(*first, sorted);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn jobs_one_forces_sequential_path() {
        let root = temp_dir("jobs-one");
        for i in 0..10 {
            write_file(&root.join(format!("{:02}.txt", i)), "x");
        }
        let opts = WalkOptions {
            jobs: Some(1),
            ..Default::default()
        };
        let seen = collect(&AppendBang, &root, &opts);
        // 10 files, all relevant.
        assert_eq!(seen.len(), 10);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn invalid_include_glob_returns_error() {
        let root = temp_dir("bad-glob");
        write_file(&root.join("a.txt"), "a");
        let opts = WalkOptions {
            include: vec!["[".into()], // unmatched bracket
            ..Default::default()
        };
        let mut visit = |_| Ok(());
        let err = walk_with(&AppendBang, &root, &opts, &mut visit).unwrap_err();
        assert!(matches!(err, Error::Config(_)), "got: {:?}", err);
        fs::remove_dir_all(&root).ok();
    }
}
