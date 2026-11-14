//! The CI formats: `--format sarif` (SARIF 2.1.0, for code scanning and
//! review tools) and `--format github` (workflow commands, which GitHub
//! Actions shows as annotations on the pull request).
//!
//! Both report each changed block of each file ([`render::findings`]) as
//! one finding, at the lines of the file as it was, with the severity of
//! the rules that changed it; conflicts are errors on their file, and
//! run-level warnings (a rule that matched nothing) have no location.
//! Neither changes what a run does: with `--check`, the exit code still
//! says whether anything would change.

use std::io::{self, Write};
use std::path::Path;

use colab_core::render::{self, Finding};
use colab_core::report::{RunReport, Severity};
use colab_core::walker::FileChange;
use serde_json::{Value, json};

use crate::format::{ExecMode, Reporter};

/// The most text of a replacement quoted in a message.
const MAX_QUOTED: usize = 400;

/// A path as a URI relative to the run's directory, `/`-separated.
fn uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.trim_start_matches("./");
    if path.is_absolute() {
        format!("file://{text}")
    } else {
        text.to_string()
    }
}

/// The rule's own `message`, or a sentence naming it.
fn message(report: &RunReport, finding: &Finding, mode: ExecMode) -> String {
    let stats: Vec<_> = finding
        .rules
        .iter()
        .filter_map(|&i| report.rules.get(i))
        .collect();
    if let Some(text) = stats.iter().find_map(|r| r.message.as_deref()) {
        return text.to_string();
    }
    let verb = match mode {
        ExecMode::Write => "changed",
        ExecMode::DryRun | ExecMode::Check => "would change",
    };
    let names: Vec<String> = stats.iter().map(|r| format!("`{}`", r.rule)).collect();
    format!("{} {verb} this", names.join(" and "))
}

/// `replacement` for a message: the new text, or that the lines go.
fn suggestion(finding: &Finding) -> String {
    if finding.replacement.is_empty() {
        return "Delete these lines.".into();
    }
    let mut text = finding.replacement.trim_end().to_string();
    if text.len() > MAX_QUOTED {
        let mut cut = MAX_QUOTED;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str(" …");
    }
    let verb = if finding.end_line < finding.start_line {
        "Insert"
    } else {
        "Replace with"
    };
    format!("{verb}:\n{text}")
}

/// The rule a finding is reported under: the most severe that touched it
/// (the first of those on a tie), so its id and the finding's level agree.
fn lead(report: &RunReport, finding: &Finding) -> Option<usize> {
    finding
        .rules
        .iter()
        .copied()
        .filter(|&i| {
            report
                .rules
                .get(i)
                .is_some_and(|r| r.severity == finding.severity)
        })
        .chain(finding.rules.first().copied())
        .next()
}

/// The changed files, kept until the run's report gives each rule's
/// severity and message.
#[derive(Default)]
struct Changes(Vec<FileChange>);

impl Changes {
    fn push(&mut self, change: &FileChange) {
        if change.changed() {
            self.0.push(change.clone());
        }
    }
}

// ---- SARIF ------------------------------------------------------------------

pub struct SarifReporter {
    mode: ExecMode,
    version: &'static str,
    changes: Changes,
    out: io::Stdout,
}

impl SarifReporter {
    pub fn new(mode: ExecMode, version: &'static str) -> Self {
        Self {
            mode,
            version,
            changes: Changes::default(),
            out: io::stdout(),
        }
    }
}

fn sarif_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
    }
}

/// A rule's SARIF id: its own `id`, or its number.
fn rule_id(report: &RunReport, index: usize) -> String {
    match report.rules.get(index).and_then(|r| r.id.as_deref()) {
        Some(id) => format!("colab/{id}"),
        None => format!("colab/rule-{}", index + 1),
    }
}

/// A SARIF region covering `start..=end` whole lines (an empty region at
/// `start` for an insertion).
fn region(finding: &Finding) -> Value {
    if finding.end_line < finding.start_line {
        json!({"startLine": finding.start_line, "startColumn": 1, "endLine": finding.start_line, "endColumn": 1})
    } else {
        json!({"startLine": finding.start_line, "startColumn": 1, "endLine": finding.end_line + 1, "endColumn": 1})
    }
}

impl SarifReporter {
    fn document(&self, report: &RunReport) -> Value {
        let rules: Vec<Value> = report
            .rules
            .iter()
            .map(|r| {
                let mut rule = json!({
                    "id": rule_id(report, r.index),
                    "name": r.rule.split_whitespace().next().unwrap_or(""),
                    "shortDescription": {"text": r.rule},
                    "defaultConfiguration": {"level": sarif_level(r.severity)},
                });
                if let Some(message) = &r.message {
                    rule["fullDescription"] = json!({"text": message});
                }
                if let Some(url) = &r.url {
                    rule["helpUri"] = json!(url);
                }
                rule
            })
            .collect();
        let mut results = Vec::new();
        for change in &self.changes.0 {
            let location = uri(&change.path);
            for finding in render::findings(change, report) {
                let Some(first) = lead(report, &finding) else {
                    continue;
                };
                // The location shows the lines as they were; an insertion
                // points at the line it goes before.
                let shown = json!({
                    "startLine": finding.start_line,
                    "endLine": finding.end_line.max(finding.start_line),
                });
                results.push(json!({
                    "ruleId": rule_id(report, first),
                    "ruleIndex": first,
                    "level": sarif_level(finding.severity),
                    "message": {"text": format!("{}. {}", message(report, &finding, self.mode), suggestion(&finding))},
                    "locations": [{"physicalLocation": {
                        "artifactLocation": {"uri": location},
                        "region": shown,
                    }}],
                    "fixes": [{
                        "description": {"text": message(report, &finding, self.mode)},
                        "artifactChanges": [{
                            "artifactLocation": {"uri": location},
                            "replacements": [{
                                "deletedRegion": region(&finding),
                                "insertedContent": {"text": finding.replacement},
                            }],
                        }],
                    }],
                }));
            }
        }
        for conflict in &report.conflicts {
            let mut result = json!({
                "ruleId": conflict.rule.map(|i| rule_id(report, i)).unwrap_or_else(|| "colab/conflict".into()),
                "level": "error",
                "message": {"text": format!("conflict: {}", conflict.message)},
                "locations": [{"physicalLocation": {"artifactLocation": {"uri": uri(&conflict.path)}}}],
            });
            if let Some(rule) = conflict.rule {
                result["ruleIndex"] = json!(rule);
            }
            results.push(result);
        }
        let notifications: Vec<Value> = render::advisories(report)
            .into_iter()
            .map(|text| json!({"level": "warning", "message": {"text": text}}))
            .collect();
        json!({
            "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
            "version": "2.1.0",
            "runs": [{
                "tool": {"driver": {
                    "name": "colab",
                    "version": self.version,
                    "informationUri": env!("CARGO_PKG_REPOSITORY"),
                    "rules": rules,
                }},
                "invocations": [{
                    "executionSuccessful": true,
                    "toolExecutionNotifications": notifications,
                }],
                "results": results,
            }],
        })
    }
}

impl Reporter for SarifReporter {
    fn report(&mut self, change: &FileChange) -> io::Result<()> {
        self.changes.push(change);
        Ok(())
    }

    fn refused(&mut self) {
        self.mode = ExecMode::DryRun;
    }

    fn finish(&mut self, report: &RunReport) -> io::Result<()> {
        writeln!(self.out, "{}", self.document(report))?;
        self.out.flush()
    }
}

// ---- GitHub workflow commands -----------------------------------------------

pub struct GithubReporter {
    mode: ExecMode,
    changes: Changes,
    out: io::Stdout,
}

impl GithubReporter {
    pub fn new(mode: ExecMode) -> Self {
        Self {
            mode,
            changes: Changes::default(),
            out: io::stdout(),
        }
    }
}

/// Escape a workflow command's message.
fn escape_data(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escape a workflow command's property value.
fn escape_property(text: &str) -> String {
    escape_data(text).replace(':', "%3A").replace(',', "%2C")
}

fn github_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "notice",
    }
}

impl GithubReporter {
    fn lines(&self, report: &RunReport) -> Vec<String> {
        let mut out = Vec::new();
        for change in &self.changes.0 {
            let file = escape_property(&uri(&change.path));
            for finding in render::findings(change, report) {
                let end = finding.end_line.max(finding.start_line);
                let lead_rule = lead(report, &finding).and_then(|i| report.rules.get(i));
                let title = lead_rule
                    .map(|r| format!("colab: {}", r.id.as_deref().unwrap_or(&r.rule)))
                    .unwrap_or_else(|| "colab".into());
                let see = lead_rule
                    .and_then(|r| r.url.as_deref())
                    .map(|url| format!(" See {url}"))
                    .unwrap_or_default();
                out.push(format!(
                    "::{} file={file},line={},endLine={end},title={}::{}",
                    github_level(finding.severity),
                    finding.start_line,
                    escape_property(&title),
                    escape_data(&format!(
                        "{}. {}{see}",
                        message(report, &finding, self.mode),
                        suggestion(&finding)
                    )),
                ));
            }
        }
        for conflict in &report.conflicts {
            out.push(format!(
                "::error file={},title=colab conflict::{}",
                escape_property(&uri(&conflict.path)),
                escape_data(&conflict.message)
            ));
        }
        for advisory in render::advisories(report) {
            out.push(format!("::warning title=colab::{}", escape_data(&advisory)));
        }
        let verb = match self.mode {
            ExecMode::Write => "changed",
            ExecMode::DryRun | ExecMode::Check => "would change",
        };
        out.push(format!(
            "colab: {} file(s) scanned, {} {verb}",
            report.files_scanned, report.files_changed
        ));
        out
    }
}

impl Reporter for GithubReporter {
    fn report(&mut self, change: &FileChange) -> io::Result<()> {
        self.changes.push(change);
        Ok(())
    }

    fn refused(&mut self) {
        self.mode = ExecMode::DryRun;
    }

    fn finish(&mut self, report: &RunReport) -> io::Result<()> {
        for line in self.lines(report) {
            writeln!(self.out, "{line}")?;
        }
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> (RunReport, FileChange) {
        let mut report =
            RunReport::with_rules(["go::import \"a\" -> \"b\"", "go::pattern 'x' -> 'y'"]);
        report.rules[0].files_matched = 1;
        report.rules[0].message = Some("a is gone, use b".into());
        report.rules[0].severity = Severity::Warning;
        report.files_scanned = 1;
        report.files_changed = 1;
        let change = FileChange {
            path: "./pkg/main.go".into(),
            before: "package p\n\nimport \"a\"\n".into(),
            after: "package p\n\nimport \"b\"\n".into(),
            rules_fired: vec![0],
            conflicts: Vec::new(),
        };
        (report, change)
    }

    #[test]
    fn a_rule_id_and_url_name_the_finding_in_both_formats() {
        let (mut report, change) = run();
        report.rules[0].id = Some("no-a".into());
        report.rules[0].url = Some("https://example.test/no-a".into());
        let mut github = GithubReporter::new(ExecMode::Check);
        github.report(&change).unwrap();
        let line = &github.lines(&report)[0];
        assert!(line.contains("title=colab%3A no-a::"), "{line}");
        assert!(line.ends_with(" See https://example.test/no-a"), "{line}");
        let mut sarif = SarifReporter::new(ExecMode::Check, "test");
        sarif.report(&change).unwrap();
        let doc = sarif.document(&report);
        let rule = &doc["runs"][0]["tool"]["driver"]["rules"][0];
        assert_eq!(rule["id"], "colab/no-a");
        assert_eq!(rule["helpUri"], "https://example.test/no-a");
        assert_eq!(doc["runs"][0]["results"][0]["ruleId"], "colab/no-a");
        // A rule without an id keeps its number.
        assert_eq!(
            doc["runs"][0]["tool"]["driver"]["rules"][1]["id"],
            "colab/rule-2"
        );
    }

    #[test]
    fn github_annotations_carry_file_line_level_and_escaped_text() {
        let (report, change) = run();
        let mut reporter = GithubReporter::new(ExecMode::Check);
        reporter.report(&change).unwrap();
        let lines = reporter.lines(&report);
        assert_eq!(
            lines[0],
            "::warning file=pkg/main.go,line=3,endLine=3,title=colab%3A go%3A%3Aimport \"a\" -> \"b\"::a is gone, use b. Replace with:%0Aimport \"b\""
        );
        // The rule that matched nothing is a warning with no file.
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("::warning title=colab::matched no files")),
            "{lines:?}"
        );
        assert_eq!(
            lines.last().unwrap(),
            "colab: 1 file(s) scanned, 1 would change"
        );
    }

    #[test]
    fn sarif_has_rules_results_fixes_and_notifications() {
        let (report, change) = run();
        let mut reporter = SarifReporter::new(ExecMode::Check, "1.2.3");
        reporter.report(&change).unwrap();
        let doc = reporter.document(&report);
        assert_eq!(doc["version"], "2.1.0");
        let run = &doc["runs"][0];
        assert_eq!(run["tool"]["driver"]["rules"][1]["id"], "colab/rule-2");
        let result = &run["results"][0];
        assert_eq!(result["ruleId"], "colab/rule-1");
        assert_eq!(result["level"], "warning");
        assert_eq!(
            result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "pkg/main.go"
        );
        assert_eq!(
            result["locations"][0]["physicalLocation"]["region"]["startLine"],
            3
        );
        let replacement = &result["fixes"][0]["artifactChanges"][0]["replacements"][0];
        assert_eq!(replacement["insertedContent"]["text"], "import \"b\"\n");
        assert_eq!(replacement["deletedRegion"]["endLine"], 4);
        assert!(
            run["invocations"][0]["toolExecutionNotifications"][0]["message"]["text"]
                .as_str()
                .unwrap()
                .contains("matched no files")
        );
    }
}
