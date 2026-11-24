//! Commands whose edits come from a language server — `colab rename` and
//! `colab mv` — and `run_change_set`, the shared report / check / write path
//! for any `ChangeSet`.

use super::*;

#[derive(Parser, Debug)]
#[command(group(clap::ArgGroup::new("target").required(true).args(["at", "symbol"])))]
pub(super) struct RenameArgs {
    /// The symbol's position: `FILE:LINE:COLUMN`, 1-based, column in
    /// characters. Exact; prefer it when you know where the definition is.
    #[arg(long, value_name = "FILE:LINE:COL", conflicts_with_all = ["symbol", "in_file"])]
    pub(super) at: Option<String>,

    /// The symbol's current name. Its first whole-word occurrence in
    /// `--in` is renamed — with every reference the server resolves to it.
    #[arg(long, value_name = "NAME", requires = "in_file")]
    pub(super) symbol: Option<String>,

    /// The file `--symbol` is looked up in.
    #[arg(long = "in", value_name = "FILE", id = "in_file")]
    pub(super) in_file: Option<PathBuf>,

    /// The new name.
    #[arg(long, value_name = "NAME")]
    pub(super) to: String,

    /// Language-server command, e.g. `"typescript-language-server --stdio"`.
    /// Defaults by file extension (rust-analyzer, gopls, clangd, …).
    #[arg(long, value_name = "CMD")]
    pub(super) lsp: Option<String>,

    /// Project root to start the server in. Defaults to the nearest
    /// ancestor of the file holding a project marker (`.git`,
    /// `Cargo.toml`, `go.mod`, `package.json`, …).
    #[arg(long, value_name = "DIR")]
    pub(super) root: Option<PathBuf>,

    /// Change to this working directory first.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    pub(super) change_dir: PathBuf,

    /// Seconds to allow for start-up, indexing, and the rename.
    #[arg(long, value_name = "SECS", default_value_t = 120)]
    pub(super) timeout: u64,

    /// Also change the old name where the server does not: whole-word
    /// matches in comments and string literals of source files, and in
    /// config and docs files (TOML, JSON, YAML, Markdown, text) under the
    /// root. Reported as its own rule. Without it they are only counted.
    #[arg(long = "include-text")]
    pub(super) include_text: bool,

    #[command(flatten)]
    pub(super) apply: ApplyFlags,
}

/// `colab mv FROM TO`.
#[derive(Parser, Debug)]
pub(super) struct MvArgs {
    /// The file or directory to move.
    pub(super) from: PathBuf,

    /// Where it goes. An existing directory means "into it", as for `mv`.
    pub(super) to: PathBuf,

    /// Language-server command that updates what refers to the moved file
    /// (`workspace/willRenameFiles`). Defaults by the file's extension.
    #[arg(long, value_name = "CMD", conflicts_with = "no_lsp")]
    pub(super) lsp: Option<String>,

    /// Move with git only; update no references.
    #[arg(long = "no-lsp")]
    pub(super) no_lsp: bool,

    /// Project root to start the server in (default: nearest project
    /// marker above FROM).
    #[arg(long, value_name = "DIR")]
    pub(super) root: Option<PathBuf>,

    /// Change to this working directory first.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    pub(super) change_dir: PathBuf,

    /// Seconds to allow for start-up, indexing, and the server's answer.
    #[arg(long, value_name = "SECS", default_value_t = 120)]
    pub(super) timeout: u64,

    #[command(flatten)]
    pub(super) apply: ApplyFlags,
}

/// Markers that identify a project root for a language server.
pub(super) const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "Cargo.toml",
    "go.mod",
    "package.json",
    "tsconfig.json",
    "pyproject.toml",
    "setup.py",
    "compile_commands.json",
    "CMakeLists.txt",
    "Package.swift",
    "Gemfile",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "composer.json",
];

pub(crate) fn project_root(file: &Path) -> Option<PathBuf> {
    file.ancestors()
        .skip(1)
        .find(|dir| PROJECT_MARKERS.iter().any(|m| dir.join(m).exists()))
        .map(Path::to_path_buf)
}

pub(super) fn run_rename(args: RenameArgs) -> Result<u8> {
    use colab_lsp_client::RenameRequest;

    change_working_dir(&args.change_dir)?;
    let cwd = std::env::current_dir().map_err(io_to_error)?;

    let place = crate::lsp_commands::resolve_place(
        args.at.as_deref(),
        args.symbol.as_deref(),
        args.in_file.as_deref(),
        &cwd,
    )?;
    let (file, offset) = (place.file, place.start);
    let server = crate::lsp_commands::server_for(args.lsp.as_deref(), &file)?;
    let root = crate::lsp_commands::root_for(args.root.as_deref(), &file, &cwd)?;

    // File operations (a class renamed with its file) only where git can
    // make and undo them; elsewhere the server is told not to send any.
    let file_operations = crate::git::Repo::discover(&root).is_some();
    info!("Renaming via `{}` in {}", server.display(), root.display());
    let started = Instant::now();
    let outcome = colab_lsp_client::rename(
        &server,
        &RenameRequest {
            root: root.clone(),
            file,
            offset,
            new_name: args.to.clone(),
            timeout: std::time::Duration::from_secs(args.timeout),
            settle: std::time::Duration::from_millis(500),
            file_operations,
        },
    )?;

    let old = outcome.old_name.as_deref().unwrap_or("?").to_string();
    let mut labels = vec![format!(
        "lsp rename `{old}` -> `{}` via {}",
        args.to, server.program
    )];
    let mut changes = outcome.changes;
    let mut warnings = Vec::new();
    if outcome.old_name.is_some() {
        let text = crate::lsp_commands::text_occurrences(&old, &args.to, &root, &changes)?;
        if args.include_text {
            labels.push(format!("text occurrences of `{old}` -> `{}`", args.to));
            crate::lsp_commands::merge_text(&mut changes, text);
        } else if !text.is_empty() {
            let summary = crate::lsp_commands::text_summary(&text, &cwd, false);
            warnings.push(format!(
                "`{old}` also appears on {} line(s) in {} file(s) the server left alone (comments, strings, config, docs), e.g. {}:{}; --include-text changes them",
                summary["lines"],
                summary["files"],
                summary["sample"][0]["path"].as_str().unwrap_or("?"),
                summary["sample"][0]["line"],
            ));
        }
    }
    let tidy = crate::lsp_commands::Tidy {
        server: &server,
        root: &root,
        timeout: args.timeout,
    };
    run_change_set(
        labels,
        changes,
        warnings,
        &args.apply,
        &cwd,
        started,
        Some(tidy),
    )
}

/// `colab mv`: ask the server what the move needs, then make it with git.
pub(super) fn run_mv(args: MvArgs) -> Result<u8> {
    change_working_dir(&args.change_dir)?;
    let cwd = std::env::current_dir().map_err(io_to_error)?;
    let started = Instant::now();
    let plan = crate::lsp_commands::plan_move(&crate::lsp_commands::MoveSpec {
        cwd: &cwd,
        from: &args.from,
        to: &args.to,
        lsp: args.lsp.as_deref(),
        no_lsp: args.no_lsp,
        root: args.root.as_deref(),
        timeout: args.timeout,
    })?;
    let mut warnings = plan.warnings;
    let tidy = plan
        .server
        .as_ref()
        .map(|server| crate::lsp_commands::Tidy {
            server,
            root: &plan.root,
            timeout: args.timeout,
        });
    if args.apply.tidy && tidy.is_none() {
        warnings.push("--tidy needs a language server; nothing was tidied".to_string());
    }
    run_change_set(
        vec![plan.label],
        plan.changes,
        warnings,
        &args.apply,
        &cwd,
        started,
        tidy,
    )
}

/// Report, and in write mode apply, a language server's answer: file
/// operations through git first, then edits — with `--check`,
/// `--backup`, and `--verify` behaving as for `refactor`.
pub(crate) fn run_change_set(
    mut labels: Vec<String>,
    mut set: colab_core::fileops::ChangeSet,
    mut warnings: Vec<String>,
    flags: &ApplyFlags,
    cwd: &Path,
    started: Instant,
    tidy: Option<crate::lsp_commands::Tidy<'_>>,
) -> Result<u8> {
    let exec_mode = if flags.verify.is_some() {
        ExecMode::Write
    } else {
        resolve_exec_mode(
            flags.write,
            flags.dry_run,
            flags.check,
            flags.format,
            io::stdout().is_terminal(),
        )
    };
    let options = crate::apply::ApplyOptions {
        allow_dirty: flags.allow_dirty,
        commit_moves: flags.commit_moves,
        backup: flags.backup.clone(),
    };
    let label = labels.first().cloned().unwrap_or_default();
    if flags.tidy
        && let Some(tidy) = &tidy
    {
        let rule = labels.len();
        labels.push(format!("tidy via {}", tidy.server.program));
        warnings.extend(crate::lsp_commands::tidy_change_set(tidy, &mut set, rule)?);
    }
    let prepared = crate::apply::prepare(&set, &options)?;
    warnings.extend(prepared.warnings.iter().cloned());

    let render_options = RenderOptions {
        detail: flags
            .detail
            .unwrap_or_else(|| flags.format.default_detail()),
        max_files: render::DEFAULT_MAX_FILES,
        max_diff_bytes: render::DEFAULT_MAX_DIFF_BYTES,
    };
    let mut reporter = format::make_reporter(flags.format, exec_mode, render_options);
    let mut report = RunReport::with_rules(labels);
    // Report paths relative to where the user is, like `refactor`.
    let relative = |p: &Path| p.strip_prefix(cwd).unwrap_or(p).to_path_buf();
    report.set_counts(set.edits.len() as u64, 0, 0);
    report.set_file_ops(set.ops.iter().map(|op| op.map_paths(relative)).collect());
    report.warn_all(warnings);
    for op in report.file_ops() {
        reporter.file_op(op).map_err(io_to_error)?;
    }

    // The server answered from the files as they were; one changed since
    // (an editor save during a slow rename) is merged, or refuses the write.
    if matches!(exec_mode, ExecMode::Write) {
        let origins: Vec<PathBuf> = set
            .edits
            .iter()
            .map(|e| set.origin_of(&e.path).unwrap_or_else(|| e.path.clone()))
            .collect();
        // A file the change set itself creates (rust-analyzer's "extract
        // module to file") is not on disk yet, so there is nothing to merge.
        let created: std::collections::HashSet<&Path> = set
            .ops
            .iter()
            .filter_map(|op| match op {
                colab_core::fileops::FileOp::Create { path } => Some(path.as_path()),
                _ => None,
            })
            .collect();
        let fresh = colab_core::merge::prepare_writes(
            set.edits
                .iter()
                .zip(&origins)
                .filter(|(e, _)| e.changed() && !created.contains(e.path.as_path()))
                .map(|(e, origin)| (origin.as_path(), e.before.as_str(), e.after.as_str())),
        )?;
        if !fresh.conflicts.is_empty() {
            report.set_conflicts(fresh.conflicts);
            report.warn(format!(
                "wrote nothing: {} file(s) changed on disk in ways that overlap this change; rerun it",
                report.conflicts().len()
            ));
            reporter.refused();
            report.set_elapsed(started.elapsed());
            reporter.finish(&report).map_err(io_to_error)?;
            return Ok(CONFLICT_EXIT);
        }
        report.warn_all(fresh.merge_warning());
        for write in fresh.writes {
            if let Some(i) = origins.iter().position(|o| *o == write.path) {
                set.edits[i].before = write.on_disk;
                set.edits[i].after = write.text;
            }
        }
    }

    let applied = if matches!(exec_mode, ExecMode::Write) {
        Some(crate::apply::apply(
            &set,
            &prepared,
            &options,
            |path, content| match &flags.backup {
                Some(dir) => write_backup_snapshot(dir, path, content),
                None => Ok(()),
            },
        )?)
    } else {
        None
    };
    for change in &set.edits {
        let mut change = change.clone();
        change.path = relative(&change.path);
        report.record(&change);
        reporter
            .report(&change)
            .map_err(|e| Error::io_at(&change.path, e))?;
    }
    if report.rules()[0].files_matched == 0 && !set.ops.is_empty() {
        // A pure move did something although it edited no file.
        report.rules_mut()[0].files_matched = set.ops.len() as u64;
    }

    if let (Some(cmd), Some(applied)) = (&flags.verify, &applied) {
        info!("Verifying with `{cmd}`");
        if !run_verify(cmd)? {
            crate::apply::revert(&set, &prepared, applied)?;
            return Err(Error::Config(format!(
                "`{label}` failed `--verify` (`{cmd}`); undid {} file operation(s) and restored {} file(s)",
                applied.done.len(),
                set.edits.len()
            )));
        }
    }

    report.set_elapsed(started.elapsed());
    reporter.finish(&report).map_err(io_to_error)?;
    let pending = report.files_changed() > 0 || !report.file_ops().is_empty();
    Ok(if matches!(exec_mode, ExecMode::Check) && pending {
        10
    } else {
        0
    })
}
