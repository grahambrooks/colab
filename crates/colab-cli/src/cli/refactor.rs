//! `colab refactor` and `colab fix`: compile the script or packs, walk the
//! targets, report, and write (or check) the result.

use super::*;

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

    refuse_phased(refactoring, args)?;
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
    let started = Instant::now();
    let mut report = refactoring.report();
    let rules_only = refactoring.report();
    let mut baseline = match &args.baseline {
        Some(reference) => Some(crate::baseline::Baseline::collect(
            reference,
            &rules_only,
            |visit| walk_targets(refactoring, args, &targets, visit).map(|_| ()),
        )?),
        None => None,
    };

    // Writes wait until the walk is over, so a conflict in any file can
    // stop all of them.
    let mut pending: Vec<FileChange> = Vec::new();
    // Changed files are recorded after `--tidy`, so the report counts what
    // would be written.
    let mut visit = |change: FileChange| -> Result<()> {
        let change = match baseline.as_mut() {
            Some(baseline) => baseline.filter(change, &rules_only),
            None => change,
        };
        if change.changed() {
            pending.push(change);
            return Ok(());
        }
        report.record(&change);
        reporter
            .report(&change)
            .map_err(|e| Error::io_at(&change.path, e))?;
        Ok(())
    };

    let outcome = if refactoring.has_scans() {
        phased_walk(refactoring, args, &targets, &mut visit)?
    } else {
        run_transformer(refactoring, args, &targets, &mut visit)?
    };
    report.record_walk(outcome);
    if let Some(baseline) = &baseline
        && baseline.dropped > 0
    {
        report.warnings.push(format!(
            "{} finding(s) left out: they are already at {} (--baseline)",
            baseline.dropped, baseline.reference
        ));
    }
    if let Some(command) = &args.tidy {
        let command = Some(command.as_str()).filter(|c| !c.is_empty());
        crate::lsp_commands::tidy_pending(command, &formatters()?, &mut pending, &mut report)?;
    }
    for change in &pending {
        report.record(change);
    }
    lsp_check(args, refactoring, &pending, &mut report)?;
    attribute_lines(args, refactoring, &pending, &mut report, reporter.as_mut());
    let prepared = match exec_mode {
        ExecMode::Write => Some(prepare_writes(&pending, &mut report)?),
        _ => None,
    };
    let blocked = prepared.is_some() && refuse_conflicts(args, &mut report);
    if blocked {
        reporter.refused();
    } else if let Some(prepared) = &prepared {
        write_changes(prepared, args.backup.as_deref())?;
    }
    report_all(reporter.as_mut(), &pending)?;
    colab_core::near_miss::probe_disk(&refactoring.rules, &mut report);
    report.elapsed = started.elapsed();
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
