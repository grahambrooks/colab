//! Runs that check a command between rules: `--commit-per-rule`,
//! `--verify`, and `--bisect`, which finds the rules a check rejects.

use super::*;

/// Apply each rule individually, with optional `--verify`
/// auto-revert and `--commit-per-rule` git history.
///
/// Always writes (the verify / commit semantics require it). On
/// verify failure, restores the failed rule's per-file backups
/// and bubbles up an error so the CLI exit code reflects the
/// failure.
pub(super) fn run_refactor_per_rule(
    refactoring: &codemod::Refactoring,
    args: &RefactorArgs,
    targets: &[PathBuf],
) -> Result<u8> {
    let mut reporter = format::make_reporter(args.format, ExecMode::Write, args.render_options());
    let started = Instant::now();
    let mut report = refactoring.report();

    for (idx, rule) in refactoring.rules.iter().enumerate() {
        let single = codemod::SingleRule::new(rule.as_ref()).in_script(refactoring.script_of(idx));
        let rule_label = rule.to_string();
        info!(
            "[{}/{}] Applying rule: {}",
            idx + 1,
            refactoring.rules.len(),
            rule_label
        );

        // Per-rule backup: only the first time we touch a file's
        // pre-rule contents go in. If the same file is re-edited
        // by a later rule, *that* rule's backup captures the
        // post-this-rule state, which is what we want for an
        // independent revert.
        let mut backups: std::collections::HashMap<PathBuf, String> =
            std::collections::HashMap::new();
        let mut pending: Vec<FileChange> = Vec::new();
        let mut visit = |mut change: FileChange| -> Result<()> {
            // `SingleRule` always reports index 0; re-index it against
            // the full script so the report reads the same as a normal run.
            change.rules_fired = change.rules_fired.into_iter().map(|_| idx).collect();
            for conflict in &mut change.conflicts {
                conflict.rule = idx;
            }
            report.record(&change);
            if change.changed() {
                pending.push(change);
                return Ok(());
            }
            reporter
                .report(&change)
                .map_err(|e| Error::io_at(&change.path, e))?;
            Ok(())
        };

        let outcome = run_transformer(&single, args, targets, &mut visit)?;
        // Each rule re-walks the same tree, so only the first pass's visits
        // and unreadable files count; the rest would multiply them.
        if idx == 0 {
            report.record_walk(outcome);
        }
        let prepared = codemod::pipeline::prepare(&pending, &mut report)?;
        if codemod::pipeline::refuse(&mut report, args.allow_conflicts, REFUSAL_HINT) {
            if idx > 0 {
                report.warn(format!(
                    "rules 1–{idx} passed and were written before rule {} conflicted; \
                 `colab undo` needs --backup",
                    idx + 1
                ));
            }
            reporter.refused();
            report_all(reporter.as_mut(), &pending)?;
            report.set_elapsed(started.elapsed());
            reporter.finish(&report).map_err(io_to_error)?;
            return Ok(CONFLICT_EXIT);
        }
        for write in &prepared.writes {
            backups
                .entry(write.path.clone())
                .or_insert_with(|| write.on_disk.clone());
        }
        codemod::pipeline::write(
            &prepared,
            &mut CliHooks::new(reporter.as_mut(), args.backup.as_deref()),
        )?;
        report_all(reporter.as_mut(), &pending)?;

        if let Some(cmd) = &args.verify {
            info!("Verifying rule {} with `{}`", idx + 1, cmd);
            if !run_verify(cmd)? {
                // Restore this rule's backups and bail.
                for (path, before) in &backups {
                    fs::write(path, before).map_err(|e| Error::io_at(path, e))?;
                }
                // Rule 1 failing alone is the signature of rules that only
                // build together, which `--verify` cannot check.
                let hint = if idx == 0 && refactoring.rules.len() > 1 {
                    "; `--verify` checks after each rule, so if the rules only build together \
                 (a new parameter and its callers), use `--bisect`, which checks once after all"
                } else {
                    ""
                };
                return Err(Error::Config(format!(
                    "rule `{}` failed `--verify` (`{}`); reverted {} file(s){hint}",
                    rule_label,
                    cmd,
                    backups.len()
                )));
            }
        }

        if args.commit_per_rule && !backups.is_empty() {
            commit_per_rule(&rule_label)?;
        }
    }

    report.set_elapsed(started.elapsed());
    reporter.finish(&report).map_err(io_to_error)?;
    Ok(0)
}

/// `command` run by the platform's shell: `sh -c`, or `cmd /C` on Windows.
pub(crate) fn shell(command: &str) -> std::process::Command {
    #[cfg(windows)]
    let mut shell = {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C");
        c
    };
    #[cfg(not(windows))]
    let mut shell = {
        let mut c = std::process::Command::new("sh");
        c.arg("-c");
        c
    };
    shell.arg(command);
    shell
}

/// Run a `--verify` shell command. Returns `true` on success.
pub(super) fn run_verify(cmd: &str) -> Result<bool> {
    let status = shell(cmd)
        .status()
        .map_err(|e| Error::Config(format!("could not spawn `{cmd}`: {e}")))?;
    Ok(status.success())
}

/// `git add -u && git commit -m "colab: <rule>"`. Bails if either
/// step fails. The "nothing to commit" case is treated as success
/// (a rule may have applied but produced no on-disk diff after
/// idempotent re-runs).
pub(super) fn commit_per_rule(rule_label: &str) -> Result<()> {
    let add = std::process::Command::new("git")
        .args(["add", "-u"])
        .status()
        .map_err(|e| Error::Config(format!("git add failed to spawn: {e}")))?;
    if !add.success() {
        return Err(Error::Config(format!(
            "git add -u failed (exit {}) for rule `{}`",
            add.code().unwrap_or(-1),
            rule_label
        )));
    }
    let message = format!("colab: {rule_label}");
    let commit = std::process::Command::new("git")
        .args(["commit", "-m", &message])
        .output()
        .map_err(|e| Error::Config(format!("git commit failed to spawn: {e}")))?;
    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr);
        // "nothing to commit" is treated as success — a rule may
        // have been a no-op even though the walker visited files.
        if stderr.contains("nothing to commit") {
            return Ok(());
        }
        return Err(Error::Config(format!(
            "git commit failed for rule `{}`: {}",
            rule_label,
            stderr.trim()
        )));
    }
    Ok(())
}

/// Apply the full refactoring, run `verify_cmd`, and on failure
/// binary-search the rule list to identify the single breaking
/// rule. Always restores the working tree to its pre-run state so
/// the user can inspect the failure cleanly.
pub(super) fn run_refactor_bisect(
    refactoring: &codemod::Refactoring,
    args: &RefactorArgs,
    targets: &[PathBuf],
    verify_cmd: &str,
) -> Result<u8> {
    if refactoring.rules.is_empty() {
        info!("No rules to bisect");
        return Ok(0);
    }

    // A tree that already fails would have one of the rules blamed for it.
    info!("Checking the starting tree with `{verify_cmd}`");
    if !run_verify(verify_cmd)? {
        return Err(Error::Config(format!(
            "the starting tree already fails `{verify_cmd}`, before any rule ran; fix it \
         (or choose a check that passes now) and rerun. Nothing was changed"
        )));
    }

    let mut reporter = format::make_reporter(args.format, ExecMode::Write, args.render_options());
    let started = Instant::now();

    // Phase 1: full apply, as an ordinary `--write` run, keeping each
    // written file's text from before. Tidied once, as a whole: the check
    // then sees formatted code, and a subset bisected later is checked
    // untidied, which a build does not mind.
    let outcome = codemod::pipeline::Pipeline::new(refactoring, source(args, targets)?)
        .write(true)
        .allow_conflicts(args.allow_conflicts)
        .refusal_hint(REFUSAL_HINT)
        .checks(checks(args)?)
        .execute(&mut CliHooks::new(
            reporter.as_mut(),
            args.backup.as_deref(),
        ))?;
    let codemod::pipeline::Outcome {
        mut report,
        changes,
        written,
        blocked,
    } = outcome;
    attribute_lines(args, refactoring, &changes, &mut report, reporter.as_mut());
    if blocked {
        reporter.refused();
        report_all(reporter.as_mut(), &changes)?;
        reporter.finish(&report).map_err(io_to_error)?;
        return Ok(CONFLICT_EXIT);
    }
    report_all(reporter.as_mut(), &changes)?;
    let snapshots: std::collections::HashMap<PathBuf, String> = written
        .into_iter()
        .map(|write| (write.path, write.on_disk))
        .collect();

    // Phase 2: full-set verify.
    info!("Verifying with `{verify_cmd}`");
    if run_verify(verify_cmd)? {
        report.set_elapsed(started.elapsed());
        reporter.finish(&report).map_err(io_to_error)?;
        return Ok(0);
    }

    // Phase 3: bisect.
    info!(
        "Verify failed; bisecting on {} rule(s)…",
        refactoring.rules.len()
    );
    let culprit_idx = bisect_rules(refactoring, &snapshots, args, targets, verify_cmd)?;
    let culprit_label = refactoring.rules[culprit_idx].to_string();

    // Phase 4: revert.
    for (path, before) in &snapshots {
        fs::write(path, before).map_err(|e| Error::io_at(path, e))?;
    }

    report.set_elapsed(started.elapsed());
    reporter.finish(&report).map_err(io_to_error)?;

    Err(Error::Config(format!(
        "rule {}/{} `{}` breaks `{}`; reverted {} file(s)",
        culprit_idx + 1,
        refactoring.rules.len(),
        culprit_label,
        verify_cmd,
        snapshots.len()
    )))
}

/// Binary search over `rules`. Invariant: `rules[..lo]` passes
/// verify, `rules[..hi]` fails. The single breaking rule is
/// `rules[lo]` once `hi - lo == 1`.
pub(super) fn bisect_rules(
    refactoring: &codemod::Refactoring,
    snapshots: &std::collections::HashMap<PathBuf, String>,
    args: &RefactorArgs,
    targets: &[PathBuf],
    verify_cmd: &str,
) -> Result<usize> {
    let rules = &refactoring.rules;
    let mut lo = 0usize;
    let mut hi = rules.len();
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        // Restore the pre-run state for every snapshotted file.
        for (path, before) in snapshots {
            fs::write(path, before).map_err(|e| Error::io_at(path, e))?;
        }
        // Apply rules[..mid] sequentially.
        for (i, rule) in rules.iter().enumerate().take(mid) {
            let single =
                codemod::SingleRule::new(rule.as_ref()).in_script(refactoring.script_of(i));
            run_transformer(
                &single,
                args,
                targets,
                &mut |change: FileChange| -> Result<()> {
                    if change.changed() {
                        fs::write(&change.path, &change.after)
                            .map_err(|e| Error::io_at(&change.path, e))?;
                    }
                    Ok(())
                },
            )?;
        }
        if run_verify(verify_cmd)? {
            info!("[bisect] rules[..{mid}] OK");
            lo = mid;
        } else {
            info!("[bisect] rules[..{mid}] FAIL");
            hi = mid;
        }
    }
    Ok(lo)
}
