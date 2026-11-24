//! One run of a compiled script, from reading files to writing them.
//!
//! Every surface that runs a script for its edits (`colab refactor`, `colab
//! fix`, `--stdin`, `--bisect`, MCP `colab.preview` / `colab.apply` /
//! `colab.test_rule`) goes through [`Pipeline::execute`], so the stages run
//! in one order everywhere:
//!
//! 1. **Walk** the [`Source`]. A script with `scan` phases is read whole and
//!    run with [`Refactoring::run_phased`]; any other walks file by file.
//!    Unchanged files are handed to [`Hooks::unchanged`] at once; changed
//!    files wait.
//! 2. **Semantic gate**: a `type` / `resolves` / `in` question that could not
//!    be answered fails the run before anything is written
//!    ([`semantic_failure`]).
//! 3. **Checks** ([`Check`]): first each one's [`Check::rewrite`] (`--tidy`),
//!    then the changes are recorded, then each one's [`Check::verify`]
//!    (the Markdown link check, `--verify-lsp`), which add conflicts.
//! 4. **Prepare** (writing runs only): every file is re-read and merged with
//!    disk changes made since the walk ([`prepare`]).
//! 5. **Refuse or write**: any conflict stops every write unless conflicts
//!    are allowed ([`refuse`]); otherwise [`write`](fn@write) writes, offering each
//!    file's disk text to [`Hooks::before_write`] first (`--backup`).
//! 6. **Probe** dead rules for near misses, and time the run.
//!
//! Runs that apply rules one at a time (`--verify`, `--commit-per-rule`,
//! `--split-pr`, the bisect search) cannot be one pass, so they compose the
//! stage functions ([`RuleRun`], [`prepare`], [`refuse`], [`write`](fn@write))
//! themselves instead of re-implementing them.
//!
//! Checks that need crates this one must not depend on (language servers,
//! the Markdown grammar) are [`Check`] objects the binary supplies, the same
//! composition-root pattern as the backend registry.

use std::path::{Path, PathBuf};
use std::time::Instant;

use colab_core::merge::Prepared;
use colab_core::walker::{self, FileChange, WalkOptions, WalkOutcome};
use colab_core::{CodeTransformer, Error, Operation, Result, RunReport};

use crate::Refactoring;

/// Where a run reads its files.
#[derive(Debug, Clone)]
pub enum Source {
    /// Walk each target (a file or a directory) with these options.
    Walk {
        targets: Vec<PathBuf>,
        options: WalkOptions,
    },
    /// Exactly these files, as `git` named them (`--changed-since`,
    /// `--staged`). A path that is no longer a file is skipped.
    Paths(Vec<PathBuf>),
    /// One text in memory under a path hint (`--stdin`, MCP
    /// `colab.test_rule`). Nothing is read from disk.
    Text { path: PathBuf, text: String },
}

impl Source {
    /// The targets walked with default options.
    pub fn walk(targets: Vec<PathBuf>) -> Self {
        Self::Walk {
            targets,
            options: WalkOptions::default(),
        }
    }

    /// Whether the run reads the disk (and so may write it, and probes it
    /// for near misses).
    pub fn on_disk(&self) -> bool {
        !matches!(self, Self::Text { .. })
    }

    /// Apply `transformer` to every relevant file, handing each result to
    /// `visit`. Unreadable files are recorded as skipped, not fatal.
    pub fn visit<T, F>(&self, transformer: &T, visit: &mut F) -> Result<WalkOutcome>
    where
        T: CodeTransformer + Sync,
        F: FnMut(FileChange) -> Result<()> + ?Sized,
    {
        match self {
            Self::Walk { targets, options } => {
                let mut outcome = WalkOutcome::default();
                for target in targets {
                    let one = walker::walk_with(transformer, target, options, &mut |c| visit(c))?;
                    outcome.files_visited += one.files_visited;
                    outcome.skipped.extend(one.skipped);
                }
                Ok(outcome)
            }
            Self::Paths(paths) => {
                let mut outcome = WalkOutcome::default();
                for path in paths {
                    if !path.is_file() {
                        continue;
                    }
                    outcome.files_visited += 1;
                    if !transformer.is_file_relevant(path) {
                        continue;
                    }
                    let before = match std::fs::read_to_string(path) {
                        Ok(before) => before,
                        // Match the tree walk: an unreadable file is recorded, not fatal.
                        Err(e) => {
                            outcome.skipped.push(colab_core::SkippedFile {
                                path: path.clone(),
                                reason: e.to_string(),
                            });
                            continue;
                        }
                    };
                    let applied = transformer.apply_at(path, &before);
                    visit(FileChange::from_outcome(path.clone(), before, applied))?;
                }
                Ok(outcome)
            }
            Self::Text { path, text } => {
                // Always applied and visited, relevant or not: the one text
                // is the run's whole output. `apply_at` gates each rule.
                let applied = transformer.apply_at(path, text);
                visit(FileChange::from_outcome(
                    path.clone(),
                    text.clone(),
                    applied,
                ))?;
                Ok(WalkOutcome {
                    files_visited: 1,
                    ..WalkOutcome::default()
                })
            }
        }
    }

    /// A script with `scan` phases over this source: every file any rule or
    /// scan applies to read into memory, the stages run over all of them,
    /// and each file's result visited as an ordinary walk would.
    pub fn visit_phased<F>(&self, refactoring: &Refactoring, visit: &mut F) -> Result<WalkOutcome>
    where
        F: FnMut(FileChange) -> Result<()> + ?Sized,
    {
        let mut files = Vec::new();
        let outcome = self.visit(&Relevant(refactoring), &mut |change: FileChange| {
            files.push((change.path, change.before));
            Ok(())
        })?;
        for change in refactoring.run_phased(files) {
            visit(change)?;
        }
        Ok(outcome)
    }
}

/// Visits every file a phased run needs (one any rule or scan applies
/// to), changing nothing: what [`Source::visit_phased`] reads into memory.
struct Relevant<'a>(&'a Refactoring);

impl CodeTransformer for Relevant<'_> {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.0.is_file_relevant(path)
            || self
                .0
                .stages
                .iter()
                .flat_map(|s| &s.scans)
                .any(|scan| scan.op.is_file_relevant(path))
    }

    fn apply(&self, source: &str) -> String {
        source.to_string()
    }
}

/// `path` as two sources can compare it: `git` names `src/a.rs` where a walk
/// of `.` yields `./src/a.rs`.
fn comparable(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

/// Run a script's `scan` phases once over `source`, leaving their facts
/// installed in the current run, for the runs that then apply its rules one
/// at a time ([`RuleRun`]): each rule's `in` reads the facts the whole
/// script's run would give it. A script without scans costs nothing.
pub fn prepare_scans(refactoring: &Refactoring, source: &Source) -> Result<()> {
    if refactoring.has_scans() {
        source.visit_phased(refactoring, &mut |_| Ok(()))?;
        semantic_failure()?;
    }
    Ok(())
}

/// A question a rule's `type` / `resolves` / `in` constraint could not have
/// answered fails the run, before anything is written.
pub fn semantic_failure() -> Result<()> {
    match colab_core::semantic::take_failure().or_else(colab_core::facts::take_failure) {
        Some(why) => Err(Error::Config(why)),
        None => Ok(()),
    }
}

/// A check over a whole run's changes, before any is written.
pub trait Check {
    /// Change the run's changes before they are recorded (`--tidy` formats
    /// them, and may add a rule to the report for the formatter).
    fn rewrite(&mut self, _changes: &mut Vec<FileChange>, _report: &mut RunReport) -> Result<()> {
        Ok(())
    }

    /// Look at the recorded changes and add a conflict to `report` for each
    /// problem (a newly broken link, a new error diagnostic).
    fn verify(
        &mut self,
        _rules: &[Box<dyn Operation>],
        _changes: &[FileChange],
        _report: &mut RunReport,
    ) -> Result<()> {
        Ok(())
    }
}

/// What a surface sees while a run goes. Every method has a default.
pub trait Hooks {
    /// Each file as the walk produced it; return what the run should treat
    /// it as (`--baseline` drops findings already present at a ref).
    fn filter(&mut self, change: FileChange) -> FileChange {
        change
    }

    /// A file the run leaves unchanged, as soon as the walk has it.
    fn unchanged(&mut self, _change: &FileChange) -> Result<()> {
        Ok(())
    }

    /// Files the walk has produced so far.
    fn progress(&mut self, _files: u64) {}

    /// The walk is over and recorded; before the checks run.
    fn walked(&mut self, _report: &mut RunReport) {}

    /// About to write `path`, which holds `on_disk` now (`--backup` saves it).
    fn before_write(&mut self, _path: &Path, _on_disk: &str) -> Result<()> {
        Ok(())
    }
}

/// [`Hooks`] that observe nothing.
pub struct NoHooks;

impl Hooks for NoHooks {}

/// What a run did.
#[derive(Debug)]
pub struct Outcome {
    pub report: RunReport,
    /// Each changed file, after the checks (`--tidy`), in walk order.
    pub changes: Vec<FileChange>,
    /// The files written, each with its text on disk before the write.
    /// Empty unless the run wrote.
    pub written: Vec<colab_core::merge::Write>,
    /// The run would have written, but conflicts stopped it.
    pub blocked: bool,
}

/// One run of a compiled script. Build it, then [`execute`](Self::execute).
pub struct Pipeline<'a> {
    refactoring: &'a Refactoring,
    source: Source,
    write: bool,
    allow_conflicts: bool,
    refusal_hint: &'a str,
    checks: Vec<Box<dyn Check + 'a>>,
    scan_source: Option<Source>,
}

impl<'a> Pipeline<'a> {
    /// A run of `refactoring` over `source` that writes nothing.
    pub fn new(refactoring: &'a Refactoring, source: Source) -> Self {
        Self {
            refactoring,
            source,
            write: false,
            allow_conflicts: false,
            refusal_hint: "fix the rules, or allow conflicts",
            checks: Vec::new(),
            scan_source: None,
        }
    }

    /// Let a script's `scan` phases read `scans` while its rules change
    /// only the files of the run's own source: `--changed-since` /
    /// `--staged` edit the files git names, but a scan must see the whole
    /// tree for `in` to mean what it means in a full run. Only a
    /// [`Source::Paths`] is narrowed this way; for any other source the
    /// scans read the run's own source, as without it.
    pub fn scan_over(mut self, scans: Source) -> Self {
        self.scan_source = Some(scans);
        self
    }

    /// Write the result (ignored for a [`Source::Text`] run).
    pub fn write(mut self, write: bool) -> Self {
        self.write = write;
        self
    }

    /// Write even when conflicts were found (never a file whose disk
    /// changes overlap the run's).
    pub fn allow_conflicts(mut self, allow: bool) -> Self {
        self.allow_conflicts = allow;
        self
    }

    /// The end of the warning a refused write leaves: how this surface
    /// overrides it (`--allow-conflicts`, `allow_conflicts: true`).
    pub fn refusal_hint(mut self, hint: &'a str) -> Self {
        self.refusal_hint = hint;
        self
    }

    /// Add a check; checks run in the order added.
    pub fn check(mut self, check: impl Check + 'a) -> Self {
        self.checks.push(Box::new(check));
        self
    }

    /// Add checks; they run in the order given, after those added before.
    pub fn checks(mut self, checks: impl IntoIterator<Item = Box<dyn Check + 'a>>) -> Self {
        self.checks.extend(checks);
        self
    }

    /// Run every stage (see the [module docs](self)).
    pub fn execute(mut self, hooks: &mut dyn Hooks) -> Result<Outcome> {
        let refactoring = self.refactoring;
        let started = Instant::now();
        let mut report = refactoring.report();

        // 1. Walk. Changed files wait for the checks and the conflict check.
        let mut pending: Vec<FileChange> = Vec::new();
        let mut seen: u64 = 0;
        let mut visit = |change: FileChange| -> Result<()> {
            let change = hooks.filter(change);
            seen += 1;
            hooks.progress(seen);
            if change.changed() {
                pending.push(change);
                return Ok(());
            }
            report.record(&change);
            hooks.unchanged(&change)
        };
        let outcome = match (&self.source, &self.scan_source) {
            // Scans over the whole tree, the run over the named files: the
            // phased run reads everything, and only the named files'
            // results are kept.
            (Source::Paths(named), Some(scans)) if refactoring.has_scans() => {
                // Each kept file is reported by the name the source gave it.
                let named: std::collections::HashMap<PathBuf, &PathBuf> =
                    named.iter().map(|p| (comparable(p), p)).collect();
                let mut kept = WalkOutcome::default();
                scans.visit_phased(refactoring, &mut |mut change: FileChange| {
                    let Some(&name) = named.get(&comparable(&change.path)) else {
                        return Ok(());
                    };
                    change.path = name.clone();
                    kept.files_visited += 1;
                    visit(change)
                })?;
                kept
            }
            _ if refactoring.has_scans() => self.source.visit_phased(refactoring, &mut visit)?,
            _ => self.source.visit(refactoring, &mut visit)?,
        };
        // 2. Nothing is written after a question that went unanswered.
        semantic_failure()?;
        report.record_walk(outcome);
        hooks.walked(&mut report);

        // 3. Checks.
        for check in &mut self.checks {
            check.rewrite(&mut pending, &mut report)?;
        }
        for change in &pending {
            report.record(change);
        }
        for check in &mut self.checks {
            check.verify(&refactoring.rules, &pending, &mut report)?;
        }

        // 4–5. Prepare, then refuse or write.
        let writes = self.write && self.source.on_disk();
        let mut written = Vec::new();
        let mut blocked = false;
        if writes {
            let prepared = prepare(&pending, &mut report)?;
            blocked = refuse(&mut report, self.allow_conflicts, self.refusal_hint);
            if !blocked {
                write(&prepared, hooks)?;
                written = prepared.writes;
            }
        }

        // 6. Probe and time.
        match &self.source {
            // The run read one text and nothing else.
            Source::Text { text, .. } => {
                colab_core::near_miss::probe(&refactoring.rules, &mut report, |_| {
                    Some(text.clone())
                });
            }
            _ => colab_core::near_miss::probe_disk(&refactoring.rules, &mut report),
        }
        report.set_elapsed(started.elapsed());
        Ok(Outcome {
            report,
            changes: pending,
            written,
            blocked,
        })
    }
}

/// One rule of a script, run alone: what `--verify`, `--commit-per-rule`,
/// `--split-pr` and the bisect search do once per rule. Every change it
/// yields is credited to the rule's index in the whole script (a
/// [`SingleRule`](crate::SingleRule) alone would say rule 0), so the
/// report reads as a normal run's.
pub struct RuleRun<'a> {
    single: crate::SingleRule<'a>,
    index: usize,
}

impl<'a> RuleRun<'a> {
    /// Rule `index` of `refactoring`.
    pub fn new(refactoring: &'a Refactoring, index: usize) -> Self {
        Self {
            single: crate::SingleRule::new(refactoring.rules[index].as_ref())
                .in_script(refactoring.script_of(index)),
            index,
        }
    }

    /// The rule's index in the script.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Whether the rule applies to `path` at all.
    pub fn is_file_relevant(&self, path: &Path) -> bool {
        self.single.is_file_relevant(path)
    }

    /// The rule over every relevant file of `source`, then the semantic gate.
    pub fn visit<F>(&self, source: &Source, visit: &mut F) -> Result<WalkOutcome>
    where
        F: FnMut(FileChange) -> Result<()>,
    {
        let outcome = source.visit(&self.single, &mut |change| visit(self.credit(change)))?;
        semantic_failure()?;
        Ok(outcome)
    }

    /// The rule over one text in memory (`--split-pr --stacked` applies a
    /// rule to what the rules before it left).
    pub fn apply_at(&self, path: &Path, text: &str) -> FileChange {
        let applied = self.single.apply_at(path, text);
        self.credit(FileChange::from_outcome(
            path.to_path_buf(),
            text.to_string(),
            applied,
        ))
    }

    fn credit(&self, mut change: FileChange) -> FileChange {
        change.rules_fired = change.rules_fired.iter().map(|_| self.index).collect();
        for conflict in &mut change.conflicts {
            conflict.rule = self.index;
        }
        change
    }
}

/// `refactoring` over one text in memory (`--stdin`, MCP
/// `colab.test_rule`): the [`Pipeline`] over a [`Source::Text`], which
/// writes nothing. Returns the text's change, and the report.
pub fn run_text(
    refactoring: &Refactoring,
    path: PathBuf,
    text: String,
) -> Result<(FileChange, RunReport)> {
    /// Catches the change if the text comes back unchanged.
    struct Keep(Option<FileChange>);
    impl Hooks for Keep {
        fn unchanged(&mut self, change: &FileChange) -> Result<()> {
            self.0 = Some(change.clone());
            Ok(())
        }
    }
    let mut keep = Keep(None);
    let outcome = Pipeline::new(refactoring, Source::Text { path, text }).execute(&mut keep)?;
    let change = outcome
        .changes
        .into_iter()
        .next()
        .or(keep.0)
        .expect("a text source visits its one text");
    Ok((change, outcome.report))
}

/// Re-read every file the run would write, before any is written
/// ([`colab_core::merge::prepare_writes`]): a file changed on disk since
/// the run read it is merged with the run's change when their edits do
/// not overlap, and is a conflict when they do, so [`refuse`] stops the
/// writes.
pub fn prepare(changes: &[FileChange], report: &mut RunReport) -> Result<Prepared> {
    let mut span = colab_core::telemetry::span("prepare writes");
    let prepared = colab_core::merge::prepare_writes(
        changes
            .iter()
            .filter(|c| c.changed())
            .map(|c| (c.path.as_path(), c.before.as_str(), c.after.as_str())),
    )?;
    span.set("colab.files.merged", prepared.merged.len());
    span.set("colab.conflicts", prepared.conflicts.len());
    report.warn_all(prepared.merge_warning());
    report.add_conflicts(prepared.conflicts.iter().cloned());
    Ok(prepared)
}

/// `true` (and a warning on the report ending with `hint`) when the run
/// found conflicts and they are not allowed, so nothing may be written.
pub fn refuse(report: &mut RunReport, allow_conflicts: bool, hint: &str) -> bool {
    if report.conflicts().is_empty() || allow_conflicts {
        return false;
    }
    report.warn(format!(
        "wrote nothing: {} conflict(s) found; {hint}",
        report.conflicts().len()
    ));
    true
}

/// Write what [`prepare`] cleared, offering each file's disk text to
/// [`Hooks::before_write`] first. A file whose disk changes conflict with
/// the run's is never written, even when conflicts are allowed.
pub fn write(prepared: &Prepared, hooks: &mut dyn Hooks) -> Result<()> {
    let mut span = colab_core::telemetry::span("write");
    span.set("colab.files.written", prepared.writes.len());
    for write in &prepared.writes {
        hooks.before_write(&write.path, &write.on_disk)?;
        std::fs::write(&write.path, &write.text).map_err(|e| Error::io_at(&write.path, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"refactor "r" { match go::import "old/pkg" { replace "new/pkg" } }"#;
    const GO: &str = "package main\n\nimport \"old/pkg\"\n\nfunc main() {}\n";

    fn compiled() -> Refactoring {
        crate::compile(SCRIPT, &colab_backends::registry()).unwrap()
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("colab-pipeline-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Records what the run showed it, and saves nothing.
    #[derive(Default)]
    struct Seen {
        unchanged: Vec<PathBuf>,
        backed_up: Vec<(PathBuf, String)>,
    }

    impl Hooks for Seen {
        fn unchanged(&mut self, change: &FileChange) -> Result<()> {
            self.unchanged.push(change.path.clone());
            Ok(())
        }

        fn before_write(&mut self, path: &Path, on_disk: &str) -> Result<()> {
            self.backed_up
                .push((path.to_path_buf(), on_disk.to_string()));
            Ok(())
        }
    }

    struct Objects;

    impl Check for Objects {
        fn verify(
            &mut self,
            _rules: &[Box<dyn Operation>],
            changes: &[FileChange],
            report: &mut RunReport,
        ) -> Result<()> {
            for change in changes {
                report.add_conflict(colab_core::report::Conflict {
                    path: change.path.clone(),
                    rule: None,
                    message: "objects".into(),
                });
            }
            Ok(())
        }
    }

    #[test]
    fn a_text_run_returns_the_change_and_reads_no_disk() {
        let refactoring = compiled();
        let (change, report) =
            run_text(&refactoring, PathBuf::from("nowhere/main.go"), GO.into()).unwrap();
        assert!(change.after.contains("new/pkg"));
        assert_eq!((report.files_visited(), report.files_changed()), (1, 1));
        // Unchanged, the text comes back as it went in.
        let (same, _) =
            run_text(&refactoring, PathBuf::from("a.go"), "package a\n".into()).unwrap();
        assert!(!same.changed());
    }

    #[test]
    fn a_writing_run_backs_up_then_writes_and_streams_unchanged_files() {
        let dir = temp("write");
        std::fs::write(dir.join("main.go"), GO).unwrap();
        std::fs::write(dir.join("other.go"), "package main\n").unwrap();
        let refactoring = compiled();
        let mut seen = Seen::default();
        let outcome = Pipeline::new(&refactoring, Source::walk(vec![dir.clone()]))
            .write(true)
            .execute(&mut seen)
            .unwrap();
        assert!(!outcome.blocked);
        assert_eq!(outcome.written.len(), 1);
        assert_eq!(seen.unchanged, vec![dir.join("other.go")]);
        assert_eq!(seen.backed_up, vec![(dir.join("main.go"), GO.to_string())]);
        assert!(
            std::fs::read_to_string(dir.join("main.go"))
                .unwrap()
                .contains("new/pkg")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_check_conflict_stops_every_write_unless_allowed() {
        let dir = temp("refuse");
        std::fs::write(dir.join("main.go"), GO).unwrap();
        let refactoring = compiled();
        let run = |allow| {
            Pipeline::new(&refactoring, Source::walk(vec![dir.clone()]))
                .write(true)
                .allow_conflicts(allow)
                .refusal_hint("say so")
                .check(Objects)
                .execute(&mut NoHooks)
                .unwrap()
        };
        let refused = run(false);
        assert!(refused.blocked && refused.written.is_empty());
        assert!(
            refused
                .report
                .warnings()
                .iter()
                .any(|w| w == "wrote nothing: 1 conflict(s) found; say so"),
            "{:?}",
            refused.report.warnings()
        );
        assert_eq!(std::fs::read_to_string(dir.join("main.go")).unwrap(), GO);
        let allowed = run(true);
        assert!(!allowed.blocked && allowed.written.len() == 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_dry_run_never_prepares_or_refuses() {
        let dir = temp("dry");
        std::fs::write(dir.join("main.go"), GO).unwrap();
        let refactoring = compiled();
        let outcome = Pipeline::new(&refactoring, Source::walk(vec![dir.clone()]))
            .check(Objects)
            .execute(&mut NoHooks)
            .unwrap();
        // The conflict is reported, but a run that writes nothing is not blocked.
        assert!(!outcome.blocked && outcome.written.is_empty());
        assert_eq!(outcome.report.conflicts().len(), 1);
        assert_eq!(outcome.changes.len(), 1);
        assert_eq!(std::fs::read_to_string(dir.join("main.go")).unwrap(), GO);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
