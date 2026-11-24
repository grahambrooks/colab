//! Runtime intermediate representation of a compiled codemod script.
//!
//! [`Refactoring`] is the value [`crate::compile`] produces. It holds a
//! list of [`Operation`]s sourced from registered language backends and
//! implements [`CodeTransformer`] so the [`colab_core::walker`] can
//! apply it without knowing about any specific language.

use std::fmt;
use std::path::Path;

use colab_core::{ApplyOutcome, CodeTransformer, Operation, RuleConflict};

/// The compiled, executable form of a codemod script.
pub struct Refactoring {
    pub name: String,
    /// Operations applied in source order. Each rule must be a no-op
    /// for files it does not care about so composition is safe across
    /// languages.
    pub rules: Vec<Box<dyn Operation>>,
    /// Each rule's `message` and `severity`, aligned with `rules`.
    pub notes: Vec<RuleNote>,
    /// The script's stages (`then { … }` starts one), each from a rule
    /// index, with the scans that run before its rules. Empty: one stage,
    /// no scans.
    pub stages: Vec<Stage>,
}

/// One file through a phased run: as it was, as it is now, the rules that
/// changed it, and each one's input (for the conflict check).
struct PhasedFile {
    path: std::path::PathBuf,
    before: String,
    text: String,
    fired: Vec<usize>,
    inputs: Vec<(usize, String)>,
    /// Files its rules would create (`create`).
    created: Vec<colab_core::Created>,
}

/// One stage of a phased run.
pub struct Stage {
    /// Its first rule; it runs to the next stage's `start`.
    pub start: usize,
    pub scans: Vec<ScanOp>,
}

/// `scan … as <name>`, compiled: the pattern that finds what it collects.
pub struct ScanOp {
    pub name: String,
    pub op: Box<dyn Operation>,
}

/// What a rule says about what it finds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleNote {
    pub message: Option<String>,
    pub severity: colab_core::Severity,
    /// `id "<name>"`, for `--only` / `--off` / `--severity`.
    pub id: Option<String>,
    /// `url "<link>"`, the rule's rationale.
    pub url: Option<String>,
    /// Whether its `where` clause asks a language server (`type`,
    /// `resolves`).
    pub semantic: bool,
    /// The script the rule came from, when it differs from the
    /// refactoring's own name (`colab fix` combines several packs).
    pub script: Option<String>,
    /// What the rule compiles to but probably should not
    /// ([`colab_core::Operation::advice`]), reported as warnings.
    pub advice: Vec<String>,
}

impl Refactoring {
    /// A report seeded with every rule, its message and its severity.
    pub fn report(&self) -> colab_core::RunReport {
        let mut report = colab_core::RunReport::with_rules(
            self.rules.iter().map(std::string::ToString::to_string),
        );
        self.annotate(&mut report);
        // Each block's advice once, though `and fences` makes two rules.
        let mut seen = std::collections::BTreeSet::new();
        for (rule, note) in self.rules.iter().zip(&self.notes) {
            for advice in &note.advice {
                if seen.insert(advice.clone()) {
                    report.warn(format!("{advice} — {rule}"));
                }
            }
        }
        report
    }

    /// The name of the script rule `index` came from: its pack's, or the
    /// refactoring's. What `colab:ignore <name>` compares against.
    pub fn script_of(&self, index: usize) -> &str {
        self.notes
            .get(index)
            .and_then(|n| n.script.as_deref())
            .unwrap_or(&self.name)
    }

    /// Several scripts as one run, in order, each rule keeping its
    /// script's name.
    pub fn combine(name: &str, parts: Vec<Refactoring>) -> Self {
        let mut rules = Vec::new();
        let mut notes = Vec::new();
        let mut stages = Vec::new();
        for part in parts {
            // Each pack's stages, from where its rules start.
            let offset = rules.len();
            for stage in part.stages {
                stages.push(Stage {
                    start: stage.start + offset,
                    scans: stage.scans,
                });
            }
            for (i, rule) in part.rules.into_iter().enumerate() {
                let mut note = part.notes.get(i).cloned().unwrap_or_default();
                note.script.get_or_insert_with(|| part.name.clone());
                rules.push(rule);
                notes.push(note);
            }
        }
        Self {
            name: name.to_string(),
            rules,
            notes,
            stages,
        }
    }

    /// Copy each rule's note onto an already seeded report.
    pub fn annotate(&self, report: &mut colab_core::RunReport) {
        for (stat, note) in report.rules_mut().iter_mut().zip(&self.notes) {
            stat.message = note.message.clone();
            stat.severity = note.severity;
            stat.id = note.id.clone();
            stat.url = note.url.clone();
        }
    }
}

/// Which rules a run keeps, and severities it overrides, from the
/// command line. A selector is a rule's `id`, a glob over ids (`no-*`),
/// or its number in the script (from 1).
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// `--only`: keep just the rules these select (all, when empty).
    pub only: Vec<String>,
    /// `--off`: drop the rules these select.
    pub off: Vec<String>,
    /// `--severity <selector>=<level>`.
    pub severity: Vec<(String, colab_core::Severity)>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.only.is_empty() && self.off.is_empty() && self.severity.is_empty()
    }
}

/// Whether `selector` names rule `index` (zero-based) with `note`.
fn selects(selector: &str, index: usize, note: &RuleNote) -> bool {
    if let Ok(n) = selector.parse::<usize>() {
        return n == index + 1;
    }
    note.id.as_deref().is_some_and(|id| glob(selector, id))
}

/// `pattern` with `*` for any run of characters, against all of `text`.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || !text[first.len()..].ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

impl Refactoring {
    /// Apply `selection`: drop the rules it leaves out and override the
    /// severities it names. A selector that selects no rule is an error
    /// naming the ids there are, so a typo never silently runs everything.
    pub fn select(&mut self, selection: &Selection) -> colab_core::Result<()> {
        let all: Vec<&String> = selection
            .only
            .iter()
            .chain(&selection.off)
            .chain(selection.severity.iter().map(|(s, _)| s))
            .collect();
        for selector in all {
            if !self
                .notes
                .iter()
                .enumerate()
                .any(|(i, n)| selects(selector, i, n))
            {
                let ids: Vec<&str> = self.notes.iter().filter_map(|n| n.id.as_deref()).collect();
                let hint = colab_core::suggest::closest(selector, ids.iter().copied())
                    .map_or_default(|id| format!(" (did you mean `{id}`?)"));
                let known = if ids.is_empty() {
                    "no rule has an `id`; select by number (1 to {n}) or give rules `id \"…\"`"
                        .replace("{n}", &self.rules.len().to_string())
                } else {
                    format!(
                        "the ids are {}, and rules are numbered 1 to {}",
                        ids.iter()
                            .map(|i| format!("`{i}`"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        self.rules.len()
                    )
                };
                return Err(colab_core::Error::Config(format!(
                    "`{selector}` selects no rule{hint}; {known}"
                )));
            }
        }
        for (i, note) in self.notes.iter_mut().enumerate() {
            if let Some((_, level)) = selection
                .severity
                .iter()
                .rev()
                .find(|(s, _)| selects(s, i, note))
            {
                note.severity = *level;
            }
        }
        let keep: Vec<bool> = self
            .notes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                (selection.only.is_empty() || selection.only.iter().any(|s| selects(s, i, n)))
                    && !selection.off.iter().any(|s| selects(s, i, n))
            })
            .collect();
        let rules = std::mem::take(&mut self.rules);
        let notes = std::mem::take(&mut self.notes);
        for ((rule, note), kept) in rules.into_iter().zip(notes).zip(keep) {
            if kept {
                self.rules.push(rule);
                self.notes.push(note);
            }
        }
        if self.rules.is_empty() {
            return Err(colab_core::Error::Config(
                "`--only` / `--off` leave no rule to run".into(),
            ));
        }
        Ok(())
    }

    /// Whether a rule asks a language server: the run must then install
    /// an oracle ([`colab_core::semantic`]), and fail on any failure.
    pub fn needs_oracle(&self) -> bool {
        self.notes.iter().any(|n| n.semantic)
    }

    /// Returns `true` if the script contains no rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Number of rules in the script.
    pub fn len(&self) -> usize {
        self.rules.len()
    }
}

impl fmt::Debug for Refactoring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Refactoring")
            .field("name", &self.name)
            .field("rules", &self.rules)
            .finish()
    }
}

impl fmt::Display for Refactoring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Refactoring '{}': [", self.name)?;
        for (i, rule) in self.rules.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{rule}")?;
        }
        write!(f, "]")
    }
}

impl CodeTransformer for Refactoring {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.rules.iter().any(|r| r.is_file_relevant(path))
    }

    /// Path-blind apply: every rule runs, relying on each one being a
    /// no-op for input it does not care about. Prefer
    /// [`apply_at`](CodeTransformer::apply_at), which the walker uses —
    /// it can skip irrelevant rules outright instead of parsing to
    /// discover they had nothing to do.
    fn apply(&self, source_code: &str) -> String {
        let mut current = source_code.to_string();
        for rule in &self.rules {
            current = rule.apply(&current);
        }
        current
    }

    /// Apply every rule that could possibly match this file, in source
    /// order, reporting which ones actually changed something.
    ///
    /// Two filters run before the expensive tree-sitter parse:
    ///
    /// 1. **Path gating** — a rule whose `is_file_relevant` rejects the
    ///    path is skipped, so a Go rule never parses a `.rs` file. This
    ///    turns the "irrelevant rules must be identity" contract from an
    ///    assumption into something the engine enforces, and it is what
    ///    makes `in "<glob>"` scoping work.
    /// 2. **Literal prefilter** — a rule whose target literal is absent
    ///    from the source cannot change it, so the parse is skipped.
    ///
    /// The prefilter tests `current`, not `source_code`, so a rule whose
    /// target was introduced by an earlier rule still fires.
    fn apply_at(&self, path: &Path, source_code: &str) -> ApplyOutcome {
        colab_core::semantic::at_path(path, || self.apply_rules_at(path, source_code))
    }
}

impl Refactoring {
    /// Whether the script has `scan` phases: then it must run as a
    /// phased run ([`run_phased`](Self::run_phased)), every file's scans
    /// before any rule of their stage.
    pub fn has_scans(&self) -> bool {
        self.stages.iter().any(|s| !s.scans.is_empty())
    }

    /// The script over `files` (path, text), stage by stage: each stage's
    /// scans read every file as the stages before left it, then its rules
    /// edit each file. Edits are still each one rule's in one file, so the
    /// result is a [`FileChange`](colab_core::FileChange) per file, judged for conflicts against
    /// the file as the whole script leaves it. The facts stay installed
    /// for whatever reads the run afterwards (`find`, the probes).
    pub fn run_phased(
        &self,
        files: Vec<(std::path::PathBuf, String)>,
    ) -> Vec<colab_core::walker::FileChange> {
        let mut state: Vec<PhasedFile> = files
            .into_iter()
            .map(|(path, text)| PhasedFile {
                path,
                before: text.clone(),
                text,
                fired: Vec::new(),
                inputs: Vec::new(),
                created: Vec::new(),
            })
            .collect();
        // Compile-time advice may have asked an `in` check before any facts
        // existed; this run answers its own.
        colab_core::facts::take_failure();
        let mut facts = colab_core::facts::Facts::default();
        let mut bounds: Vec<usize> = self.stages.iter().map(|s| s.start).collect();
        bounds.push(0);
        bounds.sort_unstable();
        bounds.dedup();
        for (k, &start) in bounds.iter().enumerate() {
            let end = bounds.get(k + 1).copied().unwrap_or(self.rules.len());
            for stage in self.stages.iter().filter(|s| s.start == start) {
                for scan in &stage.scans {
                    facts.declare(&scan.name);
                    for file in &state {
                        let (path, text) = (&file.path, &file.text);
                        if !scan.op.is_file_relevant(path)
                            || scan.op.prefilter().is_some_and(|l| !text.contains(l))
                        {
                            continue;
                        }
                        let found = colab_core::semantic::at_path(path, || scan.op.locate(text));
                        for located in found.unwrap_or_default() {
                            for (var, value) in &located.captures {
                                facts.add(&scan.name, var, value);
                            }
                        }
                    }
                }
            }
            colab_core::facts::install(facts.clone());
            for file in &mut state {
                colab_core::semantic::at_path(&file.path, || {
                    let ran = self.apply_range(&file.path, &file.text, start..end);
                    file.text = ran.text;
                    file.fired.extend(ran.fired);
                    file.inputs.extend(ran.inputs);
                    file.created.extend(ran.created);
                });
            }
        }
        state
            .into_iter()
            .flat_map(|file| {
                let conflicts = self.conflicts_of(&file.inputs, &file.text);
                colab_core::walker::FileChange::all_from_outcome(
                    file.path,
                    file.before,
                    ApplyOutcome {
                        output: file.text,
                        rules_fired: file.fired,
                        conflicts,
                        created: file.created,
                    },
                )
            })
            .collect()
    }

    /// The rules in `range` over `source`: the result, the rules that
    /// changed it, each one's input, and the files they would create.
    fn apply_range(&self, path: &Path, source: &str, range: std::ops::Range<usize>) -> Ran {
        let mut current = source.to_string();
        let mut rules_fired = Vec::new();
        let mut inputs: Vec<(usize, String)> = Vec::new();
        let mut created = Vec::new();
        for index in range {
            let rule = &self.rules[index];
            if !rule.is_file_relevant(path) {
                continue;
            }
            if let Some(literal) = rule.prefilter()
                && !current.contains(literal)
            {
                continue;
            }
            created.extend(created_by(rule.as_ref(), index, path, &current));
            let next = rule.apply(&current);
            let next = keep_ignored(self.script_of(index), rule.as_ref(), &current, next);
            if next != current {
                rules_fired.push(index);
                inputs.push((index, std::mem::replace(&mut current, next)));
            }
        }
        Ran {
            text: current,
            fired: rules_fired,
            inputs,
            created,
        }
    }

    /// Each fired rule's conflicts, judged against `after`, the file as the
    /// whole script leaves it, so a later rule that adds the missing
    /// import clears the conflict.
    fn conflicts_of(&self, inputs: &[(usize, String)], after: &str) -> Vec<RuleConflict> {
        let mut conflicts = Vec::new();
        for (index, input) in inputs {
            let rule = &self.rules[*index];
            conflicts.extend(rule.conflicts(input, after).into_iter().map(|message| {
                RuleConflict {
                    rule: *index,
                    message,
                }
            }));
        }
        conflicts
    }

    /// [`CodeTransformer::apply_at`]'s body, with semantic queries asked
    /// about `path`.
    fn apply_rules_at(&self, path: &Path, source_code: &str) -> ApplyOutcome {
        let ran = self.apply_range(path, source_code, 0..self.rules.len());
        let conflicts = self.conflicts_of(&ran.inputs, &ran.text);
        ApplyOutcome {
            output: ran.text,
            rules_fired: ran.fired,
            conflicts,
            created: ran.created,
        }
    }
}

/// What [`Refactoring::apply_range`] made of one text.
struct Ran {
    text: String,
    fired: Vec<usize>,
    inputs: Vec<(usize, String)>,
    created: Vec<colab_core::Created>,
}

/// The files `rule` (number `index`) would create from `source`, the text
/// of `path` as the rule finds it.
fn created_by(
    rule: &dyn Operation,
    index: usize,
    path: &Path,
    source: &str,
) -> Vec<colab_core::Created> {
    rule.creates(path, source)
        .into_iter()
        .map(|(path, text)| colab_core::Created {
            path,
            text,
            rule: index,
        })
        .collect()
}

/// A single-rule view over one [`Operation`], implementing
/// [`CodeTransformer`] so the walker can apply it in isolation.
///
/// Used by `colab refactor --verify` and `--commit-per-rule` to
/// run rules one at a time so failures can be attributed to (and
/// reverted at) the rule level.
pub struct SingleRule<'a> {
    pub op: &'a dyn Operation,
    /// The script's name, for `colab:ignore <name>` comments.
    pub script: &'a str,
}

impl<'a> SingleRule<'a> {
    pub fn new(op: &'a dyn Operation) -> Self {
        Self { op, script: "" }
    }

    /// The rule as part of the script `script`.
    pub fn in_script(mut self, script: &'a str) -> Self {
        self.script = script;
        self
    }
}

/// `next` (what `rule` made of `current`) with the changes that a
/// `colab:ignore` comment covers put back.
fn keep_ignored(script: &str, rule: &dyn Operation, current: &str, next: String) -> String {
    if next == current || !current.contains(colab_core::ignore::MARKER) {
        return next;
    }
    let label = rule.to_string();
    colab_core::ignore::keep_ignored(
        current,
        &next,
        colab_core::ignore::RuleNames {
            script,
            namespace: label.split_whitespace().next().unwrap_or(""),
        },
    )
}

impl CodeTransformer for SingleRule<'_> {
    fn is_file_relevant(&self, path: &Path) -> bool {
        self.op.is_file_relevant(path)
    }

    fn apply(&self, source_code: &str) -> String {
        self.op.apply(source_code)
    }

    fn apply_at(&self, path: &Path, source_code: &str) -> ApplyOutcome {
        if !self.op.is_file_relevant(path) {
            return ApplyOutcome::untraced(source_code.to_string());
        }
        if let Some(literal) = self.op.prefilter()
            && !source_code.contains(literal)
        {
            return ApplyOutcome::untraced(source_code.to_string());
        }
        let output = keep_ignored(
            self.script,
            self.op,
            source_code,
            self.op.apply(source_code),
        );
        // Index 0: the caller re-indexes this against the full script.
        let (rules_fired, conflicts) = if output == source_code {
            (Vec::new(), Vec::new())
        } else {
            let conflicts = self
                .op
                .conflicts(source_code, &output)
                .into_iter()
                .map(|message| RuleConflict { rule: 0, message })
                .collect();
            (vec![0], conflicts)
        };
        ApplyOutcome {
            output,
            rules_fired,
            conflicts,
            created: created_by(self.op, 0, path, source_code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A test operation that counts how many times `apply` ran, so tests
    /// can assert the gate and the prefilter actually avoided the
    /// (expensive) parse rather than merely producing unchanged output.
    ///
    /// `needle: None` stands in for an `ensure` rule: it has no prefilter
    /// and acts when its target is *absent*.
    #[derive(Debug)]
    struct CountingOp {
        extension: &'static str,
        needle: Option<&'static str>,
        replacement: &'static str,
        applies: Arc<AtomicUsize>,
    }

    impl CountingOp {
        /// Returns the boxed operation plus a handle to its apply counter.
        fn build(
            extension: &'static str,
            needle: Option<&'static str>,
            replacement: &'static str,
        ) -> (Box<dyn Operation>, Arc<AtomicUsize>) {
            let applies = Arc::new(AtomicUsize::new(0));
            let op = Box::new(Self {
                extension,
                needle,
                replacement,
                applies: Arc::clone(&applies),
            });
            (op, applies)
        }
    }

    impl fmt::Display for CountingOp {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "{}::test \"{}\"",
                self.extension,
                self.needle.unwrap_or("<ensure>")
            )
        }
    }

    impl Operation for CountingOp {
        fn is_file_relevant(&self, path: &Path) -> bool {
            path.extension().and_then(|s| s.to_str()) == Some(self.extension)
        }

        fn apply(&self, source_code: &str) -> String {
            self.applies.fetch_add(1, Ordering::Relaxed);
            match self.needle {
                Some(needle) => source_code.replace(needle, self.replacement),
                None if source_code.contains(self.replacement) => source_code.to_string(),
                None => format!("{}{}", source_code, self.replacement),
            }
        }

        fn prefilter(&self) -> Option<&str> {
            self.needle
        }
    }

    fn refactoring(rules: Vec<Box<dyn Operation>>) -> Refactoring {
        Refactoring {
            name: "test".to_string(),
            notes: vec![Default::default(); rules.len()],
            rules,
            stages: Vec::new(),
        }
    }

    fn count(counter: &Arc<AtomicUsize>) -> usize {
        counter.load(Ordering::Relaxed)
    }

    #[test]
    fn irrelevant_rules_are_never_applied() {
        let (go, go_applies) = CountingOp::build("go", Some("alpha"), "beta");
        let (rs, rs_applies) = CountingOp::build("rs", Some("alpha"), "gamma");

        let refactoring = refactoring(vec![go, rs]);
        let outcome = refactoring.apply_at(Path::new("x.rs"), "alpha");

        assert_eq!(outcome.output, "gamma");
        assert_eq!(outcome.rules_fired, vec![1]);
        // The Go rule never parsed the Rust file — this is the win over
        // relying on it to be a no-op.
        assert_eq!(count(&go_applies), 0);
        assert_eq!(count(&rs_applies), 1);
    }

    #[test]
    fn prefilter_skips_the_parse_when_the_literal_is_absent() {
        let (op, applies) = CountingOp::build("go", Some("needle"), "replaced");
        let refactoring = refactoring(vec![op]);

        let outcome = refactoring.apply_at(Path::new("x.go"), "nothing here");
        assert_eq!(outcome.output, "nothing here");
        assert!(outcome.rules_fired.is_empty());
        assert_eq!(count(&applies), 0);

        let outcome = refactoring.apply_at(Path::new("x.go"), "a needle b");
        assert_eq!(outcome.output, "a replaced b");
        assert_eq!(outcome.rules_fired, vec![0]);
        assert_eq!(count(&applies), 1);
    }

    #[test]
    fn ensure_style_rule_fires_when_the_target_is_absent() {
        // The regression this guards: giving an `ensure` operation a
        // prefilter would make it a no-op in exactly the files that need
        // the insert.
        let (op, applies) = CountingOp::build("go", None, "\nimport \"ctx\"");
        let refactoring = refactoring(vec![op]);

        let outcome = refactoring.apply_at(Path::new("x.go"), "package demo");
        assert_eq!(outcome.output, "package demo\nimport \"ctx\"");
        assert_eq!(outcome.rules_fired, vec![0]);
        assert_eq!(count(&applies), 1);
    }

    #[test]
    fn prefilter_sees_text_introduced_by_an_earlier_rule() {
        let (first, _) = CountingOp::build("go", Some("one"), "two");
        let (second, _) = CountingOp::build("go", Some("two"), "three");
        let refactoring = refactoring(vec![first, second]);

        let outcome = refactoring.apply_at(Path::new("x.go"), "one");
        assert_eq!(outcome.output, "three");
        assert_eq!(outcome.rules_fired, vec![0, 1]);
    }

    #[test]
    fn rules_that_change_nothing_are_not_reported_as_fired() {
        let (hit, _) = CountingOp::build("go", Some("a"), "A");
        // Present in the source, so the prefilter passes, but `apply`
        // returns the input unchanged — still not "fired".
        let (miss, miss_applies) = CountingOp::build("go", Some("z"), "z");
        let refactoring = refactoring(vec![hit, miss]);

        let outcome = refactoring.apply_at(Path::new("x.go"), "a z");
        assert_eq!(outcome.output, "A z");
        assert_eq!(outcome.rules_fired, vec![0]);
        assert_eq!(count(&miss_applies), 1);
    }

    #[test]
    fn path_blind_apply_still_runs_every_rule() {
        // `apply` keeps its old semantics for callers that have no path.
        let (go, go_applies) = CountingOp::build("go", Some("a"), "b");
        let (rs, _) = CountingOp::build("rs", Some("b"), "c");
        let refactoring = refactoring(vec![go, rs]);

        assert_eq!(refactoring.apply("a"), "c");
        assert_eq!(count(&go_applies), 1);
    }

    #[test]
    fn single_rule_reports_index_zero_and_honours_the_gate() {
        let (op, applies) = CountingOp::build("go", Some("a"), "b");
        let single = SingleRule::new(op.as_ref());

        let outcome = single.apply_at(Path::new("x.rs"), "a");
        assert_eq!(outcome.output, "a");
        assert!(outcome.rules_fired.is_empty());
        assert_eq!(count(&applies), 0);

        let outcome = single.apply_at(Path::new("x.go"), "a");
        assert_eq!(outcome.output, "b");
        assert_eq!(outcome.rules_fired, vec![0]);
        assert_eq!(count(&applies), 1);
    }
}
