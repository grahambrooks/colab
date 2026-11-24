//! `colab refactor` and `colab fix`: compile the script or packs, walk the
//! targets, report, and write (or check) the result.

use super::*;

#[derive(Parser, Debug)]
pub(super) struct RefactorArgs {
    #[command(flatten)]
    pub(super) select: SelectArgs,

    /// The language server that answers `type` / `resolves` constraints,
    /// instead of each language's usual one (`gopls`, `rust-analyzer`, …).
    #[arg(long = "lsp", value_name = "COMMAND")]
    pub(super) lsp: Option<String>,

    /// Seconds to wait for that server to start and index.
    #[arg(long = "lsp-timeout", value_name = "SECS", default_value_t = 120)]
    pub(super) lsp_timeout: u64,

    #[command(flatten)]
    pub(super) script: ScriptArgs,

    /// Change to this working directory before resolving paths.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    pub(super) change_dir: PathBuf,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub(super) format: Format,

    /// Apply changes in place. Default for `--format human` on a TTY.
    #[arg(long, conflicts_with_all = ["dry_run", "check"])]
    pub(super) write: bool,

    /// Report what would change without writing. Default for
    /// `--format json|ndjson|diff` and for non-TTY stdout.
    #[arg(long = "dry-run", conflicts_with_all = ["write", "check"])]
    pub(super) dry_run: bool,

    /// Exit 10 if any file would change. Implies `--dry-run`.
    #[arg(long, conflicts_with_all = ["write", "dry_run"])]
    pub(super) check: bool,

    /// Read source from stdin instead of walking paths. Pair with
    /// `--path` to give a filename hint for language detection.
    #[arg(long, requires = "stdin_path")]
    pub(super) stdin: bool,

    /// Path hint for `--stdin`. Drives is-this-relevant routing.
    #[arg(long = "path", value_name = "PATH", id = "stdin_path")]
    pub(super) stdin_path: Option<PathBuf>,

    /// Glob pattern to include. Repeatable. If any include is set,
    /// only matching files are processed. Uses gitignore syntax.
    #[arg(long = "include", value_name = "GLOB")]
    pub(super) include: Vec<String>,

    /// Glob pattern to exclude. Repeatable. Applied after `--include`.
    #[arg(long = "exclude", value_name = "GLOB")]
    pub(super) exclude: Vec<String>,

    /// Don't honour `.gitignore` / hidden-file rules. By default
    /// the walker behaves like `git ls-files`.
    #[arg(long = "no-ignore")]
    pub(super) no_ignore: bool,

    /// Restrict the run to files changed since the given git ref
    /// (e.g. `main`). Bypasses tree walking; iterates the git
    /// diff directly.
    #[arg(long = "changed-since", value_name = "REF")]
    pub(super) changed_since: Option<String>,

    /// Restrict the run to files in the git index (i.e. `git add`-ed).
    /// Mutually exclusive with `--changed-since`.
    #[arg(long, conflicts_with = "changed_since")]
    pub(super) staged: bool,

    /// Report only findings that are new since this git ref: the script
    /// also runs over the tree as it was there, and what it would change
    /// there too is left out. Never writes.
    #[arg(long, value_name = "REF", conflicts_with = "write")]
    pub(super) baseline: Option<String>,

    /// Worker thread count. Defaults to the available parallelism. Falls back to
    /// the `COLAB_JOBS` env var when unset. Set to 1 to force
    /// sequential processing.
    #[arg(long, value_name = "N")]
    pub(super) jobs: Option<usize>,

    /// Run this shell command after each rule's edits. A non-zero
    /// exit reverts that rule's changes and aborts the run.
    /// Implies `--write`. Every rule must pass on its own, so for rules
    /// that only work together (a new parameter and its callers) use
    /// `--bisect`, which checks once at the end.
    #[arg(long, value_name = "CMD", conflicts_with_all = ["dry_run", "check"])]
    pub(super) verify: Option<String>,

    /// After each successful rule, `git add -u && git commit -m
    /// "colab: <rule>"`. Requires a git repo. Implies `--write`.
    #[arg(long = "commit-per-rule", conflicts_with_all = ["dry_run", "check"])]
    pub(super) commit_per_rule: bool,

    /// After applying every rule, run this shell command. On
    /// failure, binary-search the rule list to identify the single
    /// breaking rule, then revert the working tree to its pre-run
    /// state and exit non-zero. Conflicts with `--verify`.
    #[arg(long, value_name = "CMD",
          conflicts_with_all = ["verify", "commit_per_rule", "dry_run", "check"])]
    pub(super) bisect: Option<String>,

    /// Before any rule writes a file, save the pre-modification
    /// contents under `<dir>`, mirroring the file's absolute path. Pair
    /// with `colab undo
    /// --from <dir>` to roll back the run.
    #[arg(long, value_name = "DIR", conflicts_with_all = ["dry_run", "check"])]
    pub(super) backup: Option<PathBuf>,

    /// Before writing, open each changed file in its language server,
    /// change it in memory, and treat every error diagnostic the change
    /// adds as a conflict. Cheaper than `--verify`'s full build, and it
    /// runs before anything is written. The server is chosen by extension
    /// (rust-analyzer, gopls, …); `--verify-lsp='<command>'` names one.
    #[arg(long = "verify-lsp", value_name = "CMD", num_args = 0..=1,
          require_equals = true, default_missing_value = "",
          conflicts_with_all = ["verify", "commit_per_rule"])]
    pub(super) verify_lsp: Option<String>,

    /// Write even when a rule reports a conflict: a `symbol` rename to a
    /// name the file already uses, or a pattern rewrite that introduces a
    /// qualifier (`fmt.`) nothing imports. Without it, a run that finds
    /// any writes nothing and exits 11; every format lists them.
    #[arg(long = "allow-conflicts")]
    pub(super) allow_conflicts: bool,

    /// After the rules, format each changed file before anything is
    /// written: with the command colab.toml's `[formatters]` gives its
    /// extension, or else its language server (organize imports, then
    /// format). Reported as its own rule (`tidy via <server or command>`)
    /// after the script's. `--tidy='<command>'` names one server for
    /// every file.
    #[arg(long, value_name = "CMD", num_args = 0..=1, require_equals = true,
          default_missing_value = "",
          conflicts_with_all = ["verify", "commit_per_rule"])]
    pub(super) tidy: Option<String>,

    /// Make one git branch per rule instead of writing: each is `HEAD`
    /// plus that rule's changes alone, named `<PREFIX>/<NN>-<rule>`
    /// (default prefix `colab/<script>`), ready for one pull request per
    /// rule. The work tree, the index and the current branch are left
    /// alone; the files the rules change must have no uncommitted changes.
    #[arg(long = "split-pr", value_name = "PREFIX", num_args = 0..=1,
          require_equals = true, default_missing_value = "",
          conflicts_with_all = ["write", "dry_run", "check", "verify", "commit_per_rule",
                                "bisect", "tidy", "backup", "stdin"])]
    pub(super) split_pr: Option<String>,

    /// With `--split-pr`: stack the branches. Each rule applies to the
    /// previous rule's output and its branch starts from the previous
    /// branch, for a chain of pull requests merged in order. A rule that
    /// only rewrites what an earlier one wrote gets a branch too.
    #[arg(long, requires = "split_pr")]
    pub(super) stacked: bool,

    /// Skip the link check. Without it, a run that changes Markdown files
    /// checks every Markdown file under the paths, and each link the run
    /// would break (a renamed heading, a moved file) is a conflict.
    #[arg(long = "no-link-check")]
    pub(super) no_link_check: bool,

    /// How much detail to report: `summary` (aggregate counters only),
    /// `counts` (adds per-rule match counts and changed paths), or
    /// `diff` (adds capped unified diffs). Defaults to `counts`, or
    /// `diff` for `--format diff`.
    #[arg(long, value_name = "LEVEL", value_parser = parse_detail)]
    pub(super) detail: Option<Detail>,

    /// Maximum number of per-file diffs to render at `--detail diff`.
    #[arg(long = "max-files", value_name = "N",
          default_value_t = render::DEFAULT_MAX_FILES)]
    pub(super) max_files: usize,

    /// Maximum size of any single rendered diff, in bytes.
    #[arg(long = "max-diff-bytes", value_name = "N",
          default_value_t = render::DEFAULT_MAX_DIFF_BYTES)]
    pub(super) max_diff_bytes: usize,

    /// Suppress per-file events; emit only the final aggregate
    /// summary. Alias for `--detail summary`.
    #[arg(long = "summary-only", conflicts_with = "detail")]
    pub(super) summary_only: bool,

    /// Files or directories to process. Defaults to the (possibly
    /// changed) working directory if none are supplied. Ignored with
    /// `--stdin`.
    pub(super) paths: Vec<PathBuf>,
}

/// `colab fix`: the packs `colab.toml` declares.
#[derive(Parser, Debug)]
pub(super) struct FixArgs {
    #[command(flatten)]
    pub(super) select: SelectArgs,

    /// Exit 10 if any pack would change a file (at a severity above
    /// `info`); write nothing. For CI.
    #[arg(long, conflicts_with_all = ["write", "dry_run"])]
    pub(super) check: bool,

    /// Apply the changes. Default for `--format human` on a TTY. With
    /// `[run] check`, the command runs afterwards, and a failure is
    /// bisected to one rule and reverted, as with `refactor --bisect`.
    #[arg(long, conflicts_with = "dry_run")]
    pub(super) write: bool,

    /// Report what would change without writing.
    #[arg(long = "dry-run")]
    pub(super) dry_run: bool,

    /// Output format, as for `refactor`.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub(super) format: Format,

    /// `summary`, `counts` or `diff`, as for `refactor`.
    #[arg(long, value_name = "LEVEL")]
    pub(super) detail: Option<String>,

    /// Start looking for `colab.toml` here instead of the working
    /// directory.
    #[arg(short = 'C', long = "change-dir", default_value = ".")]
    pub(super) change_dir: PathBuf,

    /// The config file to use, instead of the nearest `colab.toml` at or
    /// above the working directory (stopping at the repository root).
    #[arg(long, value_name = "FILE")]
    pub(super) config: Option<PathBuf>,

    /// Write even when a rule reports a conflict (see `refactor`).
    #[arg(long = "allow-conflicts")]
    pub(super) allow_conflicts: bool,

    /// Report only findings new since this git ref (see `refactor`).
    #[arg(long, value_name = "REF", conflicts_with = "write")]
    pub(super) baseline: Option<String>,

    /// Skip the Markdown link check (see `refactor`).
    #[arg(long = "no-link-check")]
    pub(super) no_link_check: bool,
}

/// `--detail` value parser. Rejects unknown levels by name rather than
/// silently falling back, and lists the valid ones.
pub(super) fn parse_detail(s: &str) -> std::result::Result<Detail, String> {
    Detail::parse(s).ok_or_else(|| {
        format!(
            "unknown detail level `{}`; expected one of: {}",
            s,
            Detail::NAMES.join(", ")
        )
    })
}

impl RefactorArgs {
    /// Resolve the rendering limits from the flags, applying the
    /// per-format default when `--detail` was not supplied.
    pub(super) fn render_options(&self) -> RenderOptions {
        let detail = if self.summary_only {
            Detail::Summary
        } else {
            self.detail.unwrap_or_else(|| self.format.default_detail())
        };
        RenderOptions {
            detail,
            max_files: self.max_files,
            max_diff_bytes: self.max_diff_bytes,
        }
    }
}

/// Pick the worker-thread count from `--jobs`, falling back to
/// `COLAB_JOBS` if the flag wasn't set. `None` lets the walker
/// default to the available parallelism.
pub(super) fn resolve_jobs(flag: Option<usize>) -> Option<usize> {
    if let Some(n) = flag {
        return Some(n);
    }
    std::env::var("COLAB_JOBS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
}

pub(super) fn run_refactor(args: RefactorArgs) -> Result<u8> {
    if args.stdin {
        return run_stdin(args);
    }

    change_working_dir(&args.change_dir)?;

    let backends = default_backends();
    let mut refactoring = args.script.compile(&backends)?;
    args.select.apply(&mut refactoring)?;
    let _oracle = OracleGuard::install(&refactoring, args.lsp.as_deref(), args.lsp_timeout)?;
    run_compiled(&refactoring, &args)
}

/// `colab fix`: compile every pack `colab.toml` names into one script and
/// run it with the file's `[run]` settings, from the file's directory.
pub(super) fn run_fix(args: FixArgs) -> Result<u8> {
    change_working_dir(&args.change_dir)?;
    let cwd = std::env::current_dir().map_err(io_to_error)?;
    let path = match &args.config {
        Some(path) => path.clone(),
        None => crate::config::find_from(&cwd).ok_or_else(|| {
            Error::Config(format!(
                "no {} at or above {} (up to the repository root); create one with a \
             `[[packs]]` table per pack, e.g. `pack = \"go/1.18-modern\"` \
             (`colab pack list` names them)",
                crate::config::FILE,
                cwd.display()
            ))
        })?,
    };
    let config = crate::config::load(&path)?;
    if config.packs.is_empty() {
        return Err(Error::Config(format!(
            "{} declares no packs; add a `[[packs]]` table with `pack = \"<built-in>\"` \
         or `path = \"<file>.colab\"`",
            path.display()
        )));
    }
    if !config.dir.as_os_str().is_empty() {
        change_working_dir(&config.dir)?;
    }

    let backends = default_backends();
    let parts = compile_packs(&config, &backends)?;
    let mut refactoring = codemod::Refactoring::combine("fix", parts);
    args.select.apply(&mut refactoring)?;
    let _oracle = OracleGuard::install(&refactoring, None, 120)?;

    // The rest is `refactor` with the config's settings as its flags.
    let format = clap::ValueEnum::to_possible_value(&args.format)
        .map_or_else(|| "human".into(), |v| v.get_name().to_string());
    // `refactor` requires a script; this one is compiled already, so the
    // placeholder rule is never read.
    let mut argv: Vec<String> = vec![
        "refactor".into(),
        "-e".into(),
        String::new(),
        "--format".into(),
        format,
    ];
    let writes = matches!(
        resolve_exec_mode(
            args.write,
            args.dry_run,
            args.check,
            args.format,
            io::stdout().is_terminal()
        ),
        ExecMode::Write
    );
    if args.check {
        argv.push("--check".into());
    } else if writes {
        argv.push("--write".into());
        if let Some(command) = &config.check {
            argv.extend(["--bisect".into(), command.clone()]);
        }
    } else {
        argv.push("--dry-run".into());
    }
    if let Some(detail) = &args.detail {
        argv.extend(["--detail".into(), detail.clone()]);
    }
    if let Some(jobs) = config.jobs {
        argv.extend(["--jobs".into(), jobs.to_string()]);
    }
    for glob in &config.exclude {
        argv.extend(["--exclude".into(), glob.clone()]);
    }
    if config.tidy {
        argv.push("--tidy".into());
    }
    if args.allow_conflicts {
        argv.push("--allow-conflicts".into());
    }
    if args.no_link_check {
        argv.push("--no-link-check".into());
    }
    if let Some(reference) = &args.baseline {
        argv.extend(["--baseline".into(), reference.clone()]);
    }
    argv.push("--".into());
    argv.extend(config.paths.iter().map(|p| p.display().to_string()));
    let refactor = RefactorArgs::try_parse_from(&argv)
        .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
    run_compiled(&refactoring, &refactor)
}

/// The `[formatters]` of the nearest `colab.toml`, for `--tidy`.
pub(super) fn formatters() -> Result<std::collections::BTreeMap<String, String>> {
    Ok(crate::config::find()?.map_or_default(|c| c.formatters))
}

/// Run an already compiled script as `refactor` would, from the working
/// directory.
pub(super) fn run_compiled(refactoring: &codemod::Refactoring, args: &RefactorArgs) -> Result<u8> {
    // The rules are in every report; the log names the script, so an
    // agent does not read each rule twice.
    info!(
        "Running `{}` ({} rule(s))",
        refactoring.name,
        refactoring.rules.len()
    );
    log::debug!("Rules: {refactoring}");

    let targets = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths.clone()
    };

    if let Some(prefix) = &args.split_pr {
        return run_split(refactoring, args, &targets, prefix);
    }

    // `--verify` and `--commit-per-rule` need per-rule visibility
    // so failures can be attributed to (and reverted at) one rule.
    if args.verify.is_some() || args.commit_per_rule {
        return run_refactor_per_rule(refactoring, args, &targets);
    }

    // `--bisect` applies the full set, runs verify, and on failure
    // binary-searches the rule list to locate the breaking rule.
    if let Some(cmd) = args.bisect.clone() {
        return run_refactor_bisect(refactoring, args, &targets, &cmd);
    }

    let stdout_is_tty = io::stdout().is_terminal();
    let exec_mode = resolve_exec_mode(
        args.write,
        args.dry_run,
        args.check,
        args.format,
        stdout_is_tty,
    );
    if args.baseline.is_some() && matches!(exec_mode, ExecMode::Write) {
        return Err(Error::Config(
            "--baseline reports findings new since a ref and never writes; drop --write, or --baseline".into(),
        ));
    }
    let mut reporter = format::make_reporter(args.format, exec_mode, args.render_options());
    let rules_only = refactoring.report();
    let mut baseline = match &args.baseline {
        Some(reference) => {
            let walk = Source::Walk {
                targets: targets.clone(),
                options: walk_options(args),
            };
            Some(crate::baseline::Baseline::collect(
                reference,
                &rules_only,
                |visit| walk.visit(refactoring, visit).map(|_| ()),
            )?)
        }
        None => None,
    };

    let mut hooks = CliHooks::new(reporter.as_mut(), args.backup.as_deref());
    hooks.baseline = baseline.as_mut().map(|b| (b, &rules_only));
    let outcome = codemod::pipeline::Pipeline::new(refactoring, source(args, &targets)?)
        .scan_over(scan_source(args, &targets))
        .write(matches!(exec_mode, ExecMode::Write))
        .allow_conflicts(args.allow_conflicts)
        .refusal_hint(REFUSAL_HINT)
        .checks(checks(args)?)
        .execute(&mut hooks)?;
    let codemod::pipeline::Outcome {
        mut report,
        changes,
        blocked,
        ..
    } = outcome;
    attribute_lines(args, refactoring, &changes, &mut report, reporter.as_mut());
    if blocked {
        reporter.refused();
    }
    report_all(reporter.as_mut(), &changes)?;
    reporter.finish(&report).map_err(io_to_error)?;

    Ok(if blocked {
        CONFLICT_EXIT
    } else if matches!(exec_mode, ExecMode::Check) && report.check_fails() {
        // Only rules above `severity info` fail a check.
        10
    } else {
        0
    })
}
