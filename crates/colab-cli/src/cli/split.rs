//! `--split-pr`: one git branch per rule, each from HEAD plus that rule's
//! changes, the working tree untouched.

use super::*;

/// A branch-name segment from free text: lowercase words joined by `-`.
pub(super) fn slug(text: &str, max: usize) -> String {
    let mut out = String::new();
    for word in text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if !out.is_empty() {
            if out.len() + 1 + word.len() > max {
                break;
            }
            out.push('-');
        }
        out.push_str(&word.to_ascii_lowercase());
    }
    out.truncate(max);
    if out.is_empty() { "rule".into() } else { out }
}

/// `refactor --split-pr`: each rule applied alone to the files as they are
/// (which must match `HEAD`), committed on its own branch from `HEAD`
/// through git plumbing. Everything is checked before any branch is made:
/// the names, uncommitted or untracked files, and conflicts.
pub(super) fn run_split(
    refactoring: &codemod::Refactoring,
    args: &RefactorArgs,
    targets: &[PathBuf],
    prefix: &str,
) -> Result<u8> {
    use std::collections::BTreeSet;
    let cwd = std::env::current_dir().map_err(io_to_error)?;
    let repo = crate::git::Repo::discover(&cwd).ok_or_else(|| {
        Error::Config("--split-pr makes git branches, so it needs a git work tree".into())
    })?;
    let base = repo.head().map_err(|_| {
        Error::Config("--split-pr branches from HEAD, and this repository has no commits".into())
    })?;
    let prefix = match prefix.trim_matches('/') {
        "" => format!("colab/{}", slug(&refactoring.name, 40)),
        given => given.to_string(),
    };
    let mut reporter = format::make_reporter(args.format, ExecMode::DryRun, args.render_options());
    let started = Instant::now();
    let mut report = refactoring.report();

    let mut scanned = BTreeSet::new();
    let mut changed_paths = BTreeSet::new();
    let mut per_rule: Vec<Vec<FileChange>> = Vec::new();
    // Bytes the branches add (or remove) between them.
    let mut delta: i64 = 0;
    // Stacked: each file as the rules so far left it. A rule reads this,
    // not the disk, for a file an earlier rule changed.
    let mut state: std::collections::BTreeMap<PathBuf, String> = Default::default();
    for (index, rule) in refactoring.rules.iter().enumerate() {
        let single =
            codemod::SingleRule::new(rule.as_ref()).in_script(refactoring.script_of(index));
        let mut changes = Vec::new();
        let mut seen_now = BTreeSet::new();
        let restack = |change: FileChange| -> FileChange {
            match state.get(&change.path) {
                Some(current) if args.stacked => {
                    let outcome = single.apply_at(&change.path, current);
                    FileChange::from_outcome(change.path, current.clone(), outcome)
                }
                _ => change,
            }
        };
        let mut visit = |change: FileChange| -> Result<()> {
            seen_now.insert(change.path.clone());
            let mut change = restack(change);
            change.rules_fired = change.rules_fired.iter().map(|_| index).collect();
            for conflict in &mut change.conflicts {
                conflict.rule = index;
            }
            if scanned.insert(change.path.clone()) {
                report.files_scanned += 1;
                report.bytes_before += change.before.len() as u64;
            }
            if change.changed() {
                changed_paths.insert(change.path.clone());
                delta += change.after.len() as i64 - change.before.len() as i64;
                changes.push(change);
            }
            Ok(())
        };
        let outcome = run_transformer(&single, args, targets, &mut visit)?;
        if index == 0 {
            report.record_walk(outcome);
        }
        // A file an earlier rule changed may now hold what this rule's
        // prefilter looks for, though the walker skipped it on disk.
        if args.stacked {
            for (path, current) in &state {
                if seen_now.contains(path) || !single.is_file_relevant(path) {
                    continue;
                }
                let outcome = single.apply_at(path, current);
                let mut change = FileChange::from_outcome(path.clone(), current.clone(), outcome);
                change.rules_fired = change.rules_fired.iter().map(|_| index).collect();
                for conflict in &mut change.conflicts {
                    conflict.rule = index;
                }
                if change.changed() {
                    changed_paths.insert(change.path.clone());
                    delta += change.after.len() as i64 - change.before.len() as i64;
                    changes.push(change);
                }
            }
            for change in &changes {
                state.insert(change.path.clone(), change.after.clone());
            }
        }
        report.rules[index].files_matched = changes.len() as u64;
        for change in &changes {
            report.conflicts.extend(change.conflicts.iter().map(|c| {
                colab_core::report::Conflict {
                    path: change.path.clone(),
                    rule: Some(index),
                    message: c.message.clone(),
                }
            }));
        }
        per_rule.push(changes);
    }
    report.bytes_after = (report.bytes_before as i64 + delta).max(0) as u64;
    report.files_changed = changed_paths.len() as u64;

    // The checks, before any branch exists.
    let mut problems = Vec::new();
    let names: Vec<String> = refactoring
        .rules
        .iter()
        .enumerate()
        .map(|(i, rule)| format!("{prefix}/{:02}-{}", i + 1, slug(&rule.to_string(), 40)))
        .collect();
    for (i, changes) in per_rule.iter().enumerate() {
        if changes.is_empty() {
            continue;
        }
        if !repo.valid_branch_name(&names[i]) {
            problems.push(format!(
                "`{}` is not a valid branch name; pass --split-pr=<prefix>",
                names[i]
            ));
        } else if repo.has_branch(&names[i]) {
            problems.push(format!(
                "branch `{}` already exists; delete it or pass --split-pr=<prefix>",
                names[i]
            ));
        }
    }
    let paths: Vec<PathBuf> = changed_paths
        .iter()
        .map(|p| fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();
    for line in repo.dirty(&paths)? {
        problems.push(format!("uncommitted changes: `{line}`; commit or stash them, so each branch is HEAD plus one rule"));
    }
    for path in &paths {
        if !repo.is_tracked(path) {
            problems.push(format!(
                "`{}` is not tracked by git, so it is not in HEAD",
                path.display()
            ));
        }
    }
    let blocked = !report.conflicts.is_empty() && !args.allow_conflicts;
    if blocked {
        report.warnings.push(format!(
            "made no branches: {} conflict(s) found; fix the rules or rerun with --allow-conflicts",
            report.conflicts.len()
        ));
    }
    if !problems.is_empty() {
        return Err(Error::Config(format!(
            "--split-pr made no branches: {}",
            problems.join("; ")
        )));
    }

    if !blocked {
        let short = &base[..base.len().min(10)];
        // Stacked, each branch starts from the one before.
        let mut parent = base.clone();
        let mut previous: Option<usize> = None;
        for (i, changes) in per_rule.iter().enumerate() {
            // A rule that changes nothing on its own gets no branch; the
            // report lists it as a rule that matched nothing.
            if changes.is_empty() {
                continue;
            }
            let label = refactoring.rules[i].to_string();
            let files: Vec<(String, &str)> = changes
                .iter()
                .map(|c| {
                    let abs = fs::canonicalize(&c.path).unwrap_or_else(|_| c.path.clone());
                    let rel = abs.strip_prefix(repo.top()).unwrap_or(&abs);
                    (rel.to_string_lossy().replace('\\', "/"), c.after.as_str())
                })
                .collect();
            let on = match previous {
                Some(p) if args.stacked => format!("on rule {}'s branch `{}`", p + 1, names[p]),
                _ if args.stacked => format!("on {short}, the first of a stack"),
                _ => format!("alone to {short}"),
            };
            let message = format!(
                "colab: {label}\n\nRule {} of {} in `{}`, applied {on} by `colab refactor --split-pr`.",
                i + 1,
                refactoring.rules.len(),
                refactoring.script_of(i)
            );
            let from = if args.stacked { &parent } else { &base };
            let commit = repo.commit_on(from, &files, &message)?;
            repo.create_branch(&names[i], &commit)?;
            if args.stacked {
                parent = commit.clone();
                previous = Some(i);
            }
            report.branches.push(colab_core::report::Branch {
                rule: i,
                name: names[i].clone(),
                commit,
                files: changes.len(),
            });
        }
    }

    // Each file once, as the rules found it.
    let mut shown = BTreeSet::new();
    for change in per_rule.iter().flatten() {
        if shown.insert(change.path.clone()) {
            reporter
                .report(change)
                .map_err(|e| Error::io_at(&change.path, e))?;
        }
    }
    colab_core::near_miss::probe_disk(&refactoring.rules, &mut report);
    report.elapsed = started.elapsed();
    reporter.finish(&report).map_err(io_to_error)?;
    Ok(if blocked { CONFLICT_EXIT } else { 0 })
}

/// For the CI formats, which annotate each changed block, record which
/// rule changed which lines (one extra apply per rule per changed file).
/// For the outputs that credit lines to rules — the CI formats, and
/// ndjson with `--detail diff` (`rule_lines`, which `colab review` reads) —
/// record which rule changed which lines (one extra apply per rule per
/// changed file), and hand that to the reporter.
pub(super) fn attribute_lines(
    args: &RefactorArgs,
    refactoring: &codemod::Refactoring,
    changes: &[FileChange],
    report: &mut RunReport,
    reporter: &mut dyn format::Reporter,
) {
    let wanted = match args.format {
        Format::Sarif | Format::Github => true,
        Format::Ndjson => args.render_options().detail == Detail::Diff,
        _ => false,
    };
    if wanted {
        colab_core::report::attribute(&refactoring.rules, changes, report);
        reporter.attribution(&report.spans);
    }
}
